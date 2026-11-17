//! New API gateway helpers (port of the Swift `NewAPIAccount` REST calls). The UI shows the gateway's login page in
//! its own webview and hands over the dashboard access token (or, for old versions, the session cookie + user id);
//! the core lists keys (masked), fetches a full key only to store it in the secret store, creates a key, logs out.
use serde_json::{json, Value};

#[derive(Default, Clone)]
pub struct Auth { pub access_token: Option<String>, pub user_id: Option<i64>, pub cookie: Option<String> }

impl Auth {
    pub fn from(p: &Value) -> Auth {
        let s = |k: &str| p[k].as_str().map(str::trim).filter(|x| !x.is_empty()).map(String::from);
        Auth { access_token: s("access_token"), user_id: p["user_id"].as_i64(), cookie: s("cookie") }
    }
}

/// `https://host[:port]` without a trailing slash; anything else is rejected.
pub fn origin(u: &str) -> Result<String, String> {
    let u = u.trim().trim_end_matches('/');
    let ok = (u.starts_with("https://") || u.starts_with("http://")) && !u.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'');
    if ok { Ok(u.to_string()) } else { Err(format!("无效的网关地址：{u}")) }
}

fn call(agent: &ureq::Agent, origin: &str, method: &str, path: &str, a: &Auth, body: Option<&Value>) -> Result<(u16, Value, String), String> {
    let mut r = agent.request(method, &format!("{origin}{path}")).set("Accept", "application/json");
    if let Some(t) = &a.access_token { r = r.set("Authorization", &format!("Bearer {t}")); }
    if let Some(u) = a.user_id { r = r.set("New-Api-User", &u.to_string()); }
    if let Some(c) = &a.cookie { r = r.set("Cookie", c); }
    let res = match body { Some(b) => r.set("Content-Type", "application/json").send_string(&b.to_string()), None => r.call() };
    let (code, text) = match res {
        Ok(resp) => (resp.status(), resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(c, resp)) => (c, resp.into_string().unwrap_or_default()),
        Err(e) => return Err(format!("无法连接网关：{e}")),
    };
    Ok((code, serde_json::from_str(&text).unwrap_or(Value::Null), text))
}

fn api_err(code: u16, v: &Value, text: &str) -> String {
    v["message"].as_str().filter(|m| !m.is_empty()).map(String::from).unwrap_or_else(|| format!("HTTP {code}: {}", crate::text::prefix(text, 200)))
}

/// `GET /api/status` → data (quota display settings etc.). Errors when this is not a New API gateway.
pub fn status(agent: &ureq::Agent, origin: &str) -> Result<Value, String> {
    let (_, v, _) = call(agent, origin, "GET", "/api/status", &Auth::default(), None)?;
    if v["success"] == true { Ok(v["data"].clone()) } else { Err("这个地址不像 New API 网关（/api/status 无有效响应）".into()) }
}

/// Show only the first / last 4 characters of a key.
pub fn mask(k: &str) -> String {
    let c: Vec<char> = k.chars().collect();
    if k.contains('*') || c.len() <= 8 { return k.to_string(); }
    format!("{}{}{}", c[..4].iter().collect::<String>(), "*".repeat(8), c[c.len() - 4..].iter().collect::<String>())
}

/// Token list (keys masked — older versions return full keys here, which never leave the core).
pub fn tokens(agent: &ureq::Agent, origin: &str, a: &Auth) -> Result<Vec<Value>, String> {
    let (code, v, text) = call(agent, origin, "GET", "/api/token/?p=1&size=100", a, None)?;
    if v["success"] != true { return Err(api_err(code, &v, &text)); }
    let items = v["data"]["items"].as_array().or(v["data"].as_array()).cloned().unwrap_or_default();
    Ok(items.into_iter().filter(|t| t["id"].is_i64()).map(|t| json!({
        "id": t["id"], "name": t["name"], "status": t["status"].as_i64().unwrap_or(1), "remain_quota": t["remain_quota"], "used_quota": t["used_quota"],
        "unlimited_quota": t["unlimited_quota"].as_bool().unwrap_or(false), "expired_time": t["expired_time"].as_i64().unwrap_or(-1),
        "model_limits_enabled": t["model_limits_enabled"], "model_limits": t["model_limits"], "group": t["group"],
        "key": mask(t["key"].as_str().unwrap_or("")),
    })).collect())
}

fn raw_list_key(agent: &ureq::Agent, origin: &str, a: &Auth, id: i64) -> Option<String> {
    let (_, v, _) = call(agent, origin, "GET", "/api/token/?p=1&size=100", a, None).ok()?;
    let items = v["data"]["items"].as_array().or(v["data"].as_array())?.clone();
    items.iter().find(|t| t["id"].as_i64() == Some(id)).and_then(|t| t["key"].as_str()).filter(|k| !k.is_empty() && !k.contains('*')).map(String::from)
}

/// Full key of one token (`POST /api/token/{id}/key`; older versions: the unmasked key from the list).
pub fn full_key(agent: &ureq::Agent, origin: &str, a: &Auth, id: i64) -> Result<String, String> {
    let (code, v, text) = call(agent, origin, "POST", &format!("/api/token/{id}/key"), a, None)?;
    let k = v["data"]["key"].as_str().filter(|k| !k.is_empty()).map(String::from)
        .or_else(|| if code == 404 { raw_list_key(agent, origin, a, id) } else { None })
        .ok_or_else(|| api_err(code, &v, &text))?;
    Ok(if k.starts_with("sk-") { k } else { format!("sk-{k}") })
}

/// New key with unlimited quota (within the user's balance), never expiring. Returns its token row.
pub fn create(agent: &ureq::Agent, origin: &str, a: &Auth, name: &str) -> Result<Value, String> {
    let body = json!({"name": name, "remain_quota": 0, "unlimited_quota": true, "expired_time": -1, "model_limits_enabled": false, "model_limits": "", "group": ""});
    let (code, v, text) = call(agent, origin, "POST", "/api/token/", a, Some(&body))?;
    if v["success"] != true { return Err(api_err(code, &v, &text)); }
    tokens(agent, origin, a)?.into_iter().filter(|t| t["name"] == name).max_by_key(|t| t["id"].as_i64().unwrap_or(0)).ok_or_else(|| api_err(code, &v, &text))
}

/// Username / password login (`POST /api/user/login`): returns the session cookie + user id (kept by the UI in memory
/// only, like the Swift version's access token). Accounts with 2FA / OAuth: paste an access token instead.
pub fn login(agent: &ureq::Agent, origin: &str, username: &str, password: &str) -> Result<Auth, String> {
    let body = json!({"username": username, "password": password});
    let res = agent.post(&format!("{origin}/api/user/login")).set("Accept", "application/json").set("Content-Type", "application/json").send_string(&body.to_string());
    let resp = match res { Ok(r) => r, Err(ureq::Error::Status(_, r)) => r, Err(e) => return Err(format!("无法连接网关：{e}")) };
    let cookie = resp.all("set-cookie").iter().filter_map(|c| c.split(';').next()).map(str::trim).filter(|c| !c.is_empty()).collect::<Vec<_>>().join("; ");
    let code = resp.status();
    let text = resp.into_string().unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if v["success"] != true { return Err(api_err(code, &v, &text)); }
    if v["data"]["require_2fa"] == true { return Err("该账号开启了两步验证，请改用访问令牌登录 / 2FA enabled: use an access token".into()); }
    Ok(Auth { access_token: None, user_id: v["data"]["id"].as_i64(), cookie: (!cookie.is_empty()).then_some(cookie) })
}

pub fn logout(agent: &ureq::Agent, origin: &str, a: &Auth) {
    let _ = call(agent, origin, "POST", "/api/user/auth/logout", a, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_and_mask() {
        assert_eq!(origin("https://gw.example.com/ ").unwrap(), "https://gw.example.com");
        assert!(origin("file:///etc").is_err() && origin("https://a b").is_err());
        assert_eq!(mask("sk-abcdefghijklmnop"), "sk-a********mnop");
        assert_eq!(mask("sk-ab**cd"), "sk-ab**cd");
    }
}
