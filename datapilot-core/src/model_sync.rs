//! Weekly sync of a gateway's "*local" models (the vLLM models an organisation serves change over time): local
//! models new on the gateway are added to the provider's model list, ones it no longer serves are removed, and a role
//! that used a removed model switches to another local model of the same provider. Only names ending in "local" are
//! touched; other models and the user's manual local marks stay as they are. State lives in config.json
//! `localModelSync.<provider id>`; `settings.localModelAutoSync = false` turns the weekly check off.
//! Which providers: `settings.localModelSyncHosts` (default: `localModelSyncHosts` of the built-in defaults, none in
//! the neutral defaults.json) — host names, IP addresses, optional
//! `:port`, `*.domain` for sub-domains; compared exactly against the host of the provider's base URL.
use serde_json::{json, Map, Value};

pub const WEEK_SECS: f64 = 7.0 * 86400.0;

/// Gateways whose local models are synced when the settings name none (built-in defaults).
pub fn default_hosts() -> Vec<String> { crate::defaults::get().local_model_sync_hosts.clone() }

/// Gateways whose local models are synced: `settings.localModelSyncHosts` when set (an empty list turns it off for
/// every provider), else the built-in defaults.
pub fn hosts(raw: &Value) -> Vec<String> {
    match raw["settings"]["localModelSyncHosts"].as_array() {
        Some(a) => a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => default_hosts(),
    }
}

/// (host, port) of a URL or a `host[:port]` entry: lower-case, IPv6 without brackets. With `default_port` an
/// http(s) URL without a port gets 80 / 443 (base URLs); entries only carry the port written in them.
pub(crate) fn host_port(s: &str, default_port: bool) -> Option<(String, Option<u16>)> {
    let s = s.trim();
    let (scheme, rest) = match s.split_once("://") { Some((a, b)) => (a.to_lowercase(), b), None => (String::new(), s) };
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit_once('@').map(|x| x.1).unwrap_or(auth);
    let (host, port) = if let Some(r) = auth.strip_prefix('[') {
        let (h, after) = r.split_once(']')?;
        (h, after.strip_prefix(':'))
    } else if auth.matches(':').count() > 1 {
        (auth, None)   // bare IPv6 address
    } else {
        match auth.split_once(':') { Some((h, p)) => (h, Some(p)), None => (auth, None) }
    };
    let port = match port.filter(|p| !p.is_empty()) { Some(p) => Some(p.parse::<u16>().ok()?), None => None };
    let port = port.or(if !default_port { None } else if scheme == "https" { Some(443) } else if scheme == "http" { Some(80) } else { None });
    let host = host.trim_end_matches('.').to_lowercase();
    if host.is_empty() { None } else { Some((host, port)) }
}

/// The provider's base URL matches one entry exactly: same host (IP addresses compared as addresses; `*.x` = any
/// sub-domain of x), and the same port when the entry names one.
pub fn is_target(base_url: &str, hosts: &[String]) -> bool {
    let Some((uh, up)) = host_port(base_url, true) else { return false };
    let uip = uh.parse::<std::net::IpAddr>().ok();
    hosts.iter().filter_map(|e| host_port(e, false)).any(|(eh, ep)| {
        let host_ok = match (eh.parse::<std::net::IpAddr>().ok(), uip) {
            (Some(a), Some(b)) => a == b,
            _ => eh == uh || eh.strip_prefix("*.").map(|d| uh.ends_with(&format!(".{d}"))).unwrap_or(false),
        };
        host_ok && ep.map(|p| Some(p) == up).unwrap_or(true)
    })
}

/// Same rule as the UI's `modelKind` (models.ts).
pub fn model_kind(m: &str) -> &'static str {
    let l = m.to_lowercase();
    if l.contains("rerank") { return "rerank"; }
    if l.contains("embed") || l.starts_with("bge-") || l.contains("bge-m3") || l.contains("text-embedding") { return "embedding"; }
    if ["whisper", "tts", "speech", "image", "seedream", "seedance", "video", "t2v", "i2v", "r2v", "dall-e", "sora", "midjourney"].iter().any(|k| l.contains(k)) { return "other"; }
    "chat"
}

pub fn is_local_chat(m: &str) -> bool { model_kind(m) == "chat" && m.to_lowercase().ends_with("local") }

pub fn auto_enabled(raw: &Value) -> bool { raw["settings"]["localModelAutoSync"].as_bool().unwrap_or(true) }

/// Never checked successfully, or the last successful check is at least a week old.
pub fn due(raw: &Value, pid: &str, now: f64) -> bool { raw["localModelSync"][pid]["lastCheck"].as_f64().map(|t| now - t >= WEEK_SECS).unwrap_or(true) }

/// Same rule as the UI's `defaultLocalModel`: prefer qwen3, else the first by name.
pub fn default_local(models: &[String]) -> Option<String> {
    let mut c: Vec<&String> = models.iter().filter(|m| is_local_chat(m)).collect();
    c.sort();
    c.iter().find(|m| m.to_lowercase().contains("qwen3")).or(c.first()).map(|s| s.to_string())
}

fn role_name(r: &str) -> &'static str { match r { "main" => "主模型", "fast" => "快速模型", _ => "视觉模型" } }

#[derive(Debug, Default)]
pub struct Plan {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// (role, from, to)
    pub switched: Vec<(String, String, String)>,
    /// roles whose model was removed with no local model left to switch to (left unchanged)
    pub stranded: Vec<String>,
    /// merge patch for config.json
    pub patch: Value,
}

impl Plan {
    pub fn changed(&self) -> bool { !self.added.is_empty() || !self.removed.is_empty() }

    pub fn message(&self, provider: &str) -> String {
        let mut parts = vec![];
        if !self.added.is_empty() { parts.push(format!("新增 {}", self.added.join("、"))); }
        if !self.removed.is_empty() { parts.push(format!("移除 {}", self.removed.join("、"))); }
        for (r, _, to) in &self.switched { parts.push(format!("{}已切换为 {to}", role_name(r))); }
        for r in &self.stranded { parts.push(format!("{}使用的模型已下线且没有可替换的本地模型，请在 设置 → 模型角色 中重新选择", role_name(r))); }
        format!("本地模型已更新（{provider}）：{} / Local models updated", parts.join("；"))
    }
}

fn strs(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

/// What to change for provider `pid` given the gateway's current model list `remote` (must be non-empty: an empty or
/// failed listing changes nothing).
pub fn plan(raw: &Value, pid: &str, remote: &[String], now: f64) -> Plan {
    let cache = strs(&raw["modelCache"][pid]);
    let remote_local: Vec<String> = remote.iter().filter(|m| is_local_chat(m)).cloned().collect();
    // local models already offered to the user (last sync, else the list fetched when the provider was added): only
    // ones new on the gateway are added, so a local model the user chose not to add is not forced back in
    let st = &raw["localModelSync"][pid]["known"];
    let known: Vec<String> = if st.is_array() { strs(st) } else { strs(&raw["availableModels"][pid]).into_iter().filter(|m| is_local_chat(m)).collect() };
    let added: Vec<String> = remote_local.iter().filter(|m| !known.contains(m) && !cache.contains(m)).cloned().collect();
    let removed: Vec<String> = cache.iter().filter(|m| is_local_chat(m) && !remote.contains(m)).cloned().collect();
    let mut new_cache: Vec<String> = cache.iter().filter(|m| !removed.contains(m)).cloned().collect();
    new_cache.extend(added.iter().cloned());

    let mut marks = Map::new();
    for m in &added {
        let k = format!("{pid}|{m}");
        if raw["settings"]["localMarks"].get(&k).is_none() { marks.insert(k, json!(true)); }
    }
    for m in &removed { marks.insert(format!("{pid}|{m}"), Value::Null); }

    let (mut roles, mut switched, mut stranded) = (Map::new(), vec![], vec![]);
    for role in ["main", "fast", "vision"] {
        let r = &raw["roles"][role];
        if r["providerId"].as_str() != Some(pid) { continue; }
        let Some(from) = r["model"].as_str().filter(|m| removed.iter().any(|x| x == m)) else { continue };
        match default_local(&new_cache) {
            Some(to) => {
                roles.insert(role.into(), json!({"providerId": pid, "model": to, "capabilities": null}));
                switched.push((role.to_string(), from.to_string(), to));
            }
            None => stranded.push(role.to_string()),
        }
    }

    let mut patch = json!({
        "localModelSync": {pid: {"lastCheck": now, "lastTry": now, "ok": true, "error": null, "added": added, "removed": removed, "known": remote_local}},
        "availableModels": {pid: remote},
    });
    if !added.is_empty() || !removed.is_empty() { patch["modelCache"] = json!({pid: new_cache}); }
    if !marks.is_empty() { patch["settings"] = json!({"localMarks": marks}); }
    if !roles.is_empty() { patch["roles"] = Value::Object(roles); }
    Plan { added, removed, switched, stranded, patch }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn adds_new_removes_gone_keeps_others() {
        let raw = json!({
            "modelCache": {"p": ["glm-5.3-flash", "qwen3.8-27b-local", "old-local", "Hy4-preview"]},
            "availableModels": {"p": ["glm-5.3-flash", "qwen3.8-27b-local", "old-local", "skipped-local", "bge-local"]},
            "settings": {"localMarks": {"p|Hy4-preview": true, "p|old-local": true}},
            "roles": {"main": {"providerId": "p", "model": "old-local", "capabilities": {"contextWindow": 1}},
                      "fast": {"providerId": "p", "model": "glm-5.3-flash"}},
        });
        // skipped-local: offered before, not chosen → not re-added; Hy4-preview (manual mark, not *local) untouched
        let remote = v(&["glm-5.3-flash", "qwen3.8-27b-local", "skipped-local", "qwen3.9-32b-local", "Qwen3-Embedding-local"]);
        let p = plan(&raw, "p", &remote, 100.0);
        assert_eq!(p.added, v(&["qwen3.9-32b-local"]));
        assert_eq!(p.removed, v(&["old-local"]));
        assert_eq!(p.patch["modelCache"]["p"], json!(["glm-5.3-flash", "qwen3.8-27b-local", "Hy4-preview", "qwen3.9-32b-local"]));
        assert_eq!(p.patch["settings"]["localMarks"], json!({"p|qwen3.9-32b-local": true, "p|old-local": null}));
        // main switched to a remaining local model (qwen3 first, by name), its saved caps dropped; fast untouched
        assert_eq!(p.switched, vec![("main".to_string(), "old-local".to_string(), "qwen3.8-27b-local".to_string())]);
        assert_eq!(p.patch["roles"], json!({"main": {"providerId": "p", "model": "qwen3.8-27b-local", "capabilities": null}}));
        assert_eq!(p.patch["localModelSync"]["p"]["known"], json!(["qwen3.8-27b-local", "skipped-local", "qwen3.9-32b-local"]));
        assert!(p.message("Gateway").contains("主模型已切换为 qwen3.8-27b-local"));

        // next week: nothing new → only the check state is written
        let mut raw2 = raw.clone();
        crate::server::ui_rpc::merge(&mut raw2, &p.patch);
        let p2 = plan(&raw2, "p", &remote, 200.0);
        assert!(!p2.changed() && p2.patch.get("modelCache").is_none() && p2.patch.get("roles").is_none());
        assert!(!due(&raw2, "p", 100.0 + WEEK_SECS - 1.0) && due(&raw2, "p", 100.0 + WEEK_SECS));
    }

    #[test]
    fn stranded_role_left_alone() {
        let raw = json!({"modelCache": {"p": ["a-local", "deepseek-chat"]}, "roles": {"main": {"providerId": "p", "model": "a-local"}}});
        let p = plan(&raw, "p", &v(&["deepseek-chat"]), 1.0);
        assert_eq!(p.removed, v(&["a-local"]));
        assert_eq!(p.stranded, v(&["main"]));
        assert!(p.patch.get("roles").is_none());
        assert!(hosts(&json!({})).is_empty());   // neutral built-in defaults: no gateway
        let gw = hosts(&json!({"settings": {"localModelSyncHosts": ["ai.gateway.example.edu"]}}));
        assert!(is_target("https://ai.gateway.example.edu:3443/v1", &gw) && is_target("HTTPS://AI.Gateway.Example.edu/v1", &gw));
        assert!(!is_target("https://ai.gateway.example.edu.example.com/v1", &gw) && !is_target("https://x.com/ai.gateway.example.edu", &gw));
        assert!(!is_target("http://localhost:11434/v1", &gw) && hosts(&json!({"settings": {"localModelSyncHosts": []}})).is_empty());
        let mine: Vec<String> = ["10.20.0.5:8000", "*.corp.example", "[fd00::5]", "https://relay.example.org/v1"].map(String::from).to_vec();
        assert!(is_target("http://10.20.0.5:8000/v1", &mine) && !is_target("http://10.20.0.5:9000/v1", &mine) && !is_target("http://10.20.0.50:8000/v1", &mine));
        assert!(is_target("https://ai.corp.example/v1", &mine) && !is_target("https://corp.example/v1", &mine) && !is_target("https://evilcorp.example/v1", &mine));
        assert!(is_target("http://[fd00:0::5]:8080/v1", &mine) && is_target("https://relay.example.org:8443/v1", &mine));
        assert!(auto_enabled(&json!({})) && !auto_enabled(&json!({"settings": {"localModelAutoSync": false}})));
    }
}
