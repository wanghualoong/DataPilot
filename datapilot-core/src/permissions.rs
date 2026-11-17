//! Permission modes, rules and the decision chain (port of the Swift `PermissionEngine`, same semantics).
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Mode { Plan, Ask, Auto, Full }

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s { "plan" => Some(Mode::Plan), "ask" => Some(Mode::Ask), "auto" => Some(Mode::Auto), "full" => Some(Mode::Full), _ => None }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Risk {
    #[serde(rename = "readOnly")] ReadOnly,
    #[serde(rename = "sandboxed")] Sandboxed,
    #[serde(rename = "network")] Network,
    #[serde(rename = "external")] External,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self { Risk::ReadOnly => "readOnly", Risk::Sandboxed => "sandboxed", Risk::Network => "network", Risk::External => "external" }
    }
    pub fn parse(s: &str) -> Risk {
        match s { "readOnly" => Risk::ReadOnly, "network" => Risk::Network, "external" => Risk::External, _ => Risk::Sandboxed }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Behavior { Allow, Ask, Deny }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rule { pub tool: String, #[serde(default)] pub pattern: Option<String>, pub behavior: Behavior }

impl Rule {
    pub fn display(&self) -> String { match &self.pattern { Some(p) => format!("{}({p})", self.tool), None => self.tool.clone() } }

    pub fn matches(&self, tool: &str, args: &Value, summary: &str) -> bool {
        let name_ok = self.tool == "*" || self.tool == tool || (self.tool.ends_with('*') && tool.starts_with(&self.tool[..self.tool.len() - 1]));
        if !name_ok { return false; }
        let p = match self.pattern.as_deref().map(str::trim) { Some(p) if !p.is_empty() => p, _ => return true };
        if let Some(d) = p.strip_prefix("domain:") {
            let d = d.to_lowercase();
            return match host(args) { Some(h) => { let h = h.to_lowercase(); h == d || h.ends_with(&format!(".{d}")) } None => false };
        }
        let subject = subject(args).unwrap_or_else(|| summary.to_string());
        if let Some(pre) = p.strip_suffix('*') { return subject.to_lowercase().starts_with(&pre.trim().to_lowercase()); }
        let pl = p.to_lowercase();
        subject.to_lowercase().contains(&pl) || summary.to_lowercase().contains(&pl)
    }
}

pub fn host(a: &Value) -> Option<String> {
    let s = a.get("url")?.as_str()?;
    let s = s.split("://").nth(1).unwrap_or(s);
    let h = s.split(['/', '?', '#']).next()?.split('@').last()?.split(':').next()?;
    if h.is_empty() { None } else { Some(h.to_string()) }
}

pub fn subject(a: &Value) -> Option<String> {
    ["command", "path", "url", "action"].iter().find_map(|k| a.get(*k).and_then(|v| v.as_str()).map(String::from))
}

pub fn suggest(args: &Value) -> Option<String> {
    if let Some(h) = host(args) { return Some(format!("domain:{h}")); }
    if let Some(c) = args.get("command").and_then(|v| v.as_str()) {
        let w: Vec<&str> = c.split(' ').filter(|x| !x.is_empty()).take(2).collect();
        if !w.is_empty() { return Some(format!("{} *", w.join(" "))); }
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision { Allow, Ask, Deny(String) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice { AllowOnce, AllowSession, AllowAlways, Deny }

impl Choice {
    pub fn parse(s: &str) -> Choice {
        match s { "allowOnce" | "allow_once" | "allow" => Choice::AllowOnce, "allowSession" | "allow_session" => Choice::AllowSession,
                  "allowAlways" | "allow_always" => Choice::AllowAlways, _ => Choice::Deny }
    }
}

pub const PLAN_BLOCKED_PREFIX: &str = "[计划模式]";

pub struct Engine { pub mode: Mutex<Mode>, pub rules: Mutex<Vec<Rule>>, session_allows: Mutex<Vec<Rule>> }

/// What the engine needs to know about a tool for one call.
pub struct ToolView<'a> { pub name: &'a str, pub risk: Risk, pub plan_mode: bool, pub summary: &'a str }

impl Engine {
    pub fn new(mode: Mode, rules: Vec<Rule>) -> Self { Engine { mode: Mutex::new(mode), rules: Mutex::new(rules), session_allows: Mutex::new(vec![]) } }
    pub fn mode(&self) -> Mode { *self.mode.lock().unwrap() }
    pub fn set_mode(&self, m: Mode) { *self.mode.lock().unwrap() = m; }

    /// deny rules → plan gate → ask rules → allow rules / session allows → mode × risk default.
    pub fn decide(&self, t: &ToolView, args: &Value) -> Decision {
        let mode = self.mode();
        let rules = self.rules.lock().unwrap();
        let hit = |b: Behavior| rules.iter().find(|r| r.behavior == b && r.matches(t.name, args, t.summary)).cloned();
        if let Some(r) = hit(Behavior::Deny) { return Decision::Deny(format!("blocked by rule: {}", r.display())); }
        if mode == Mode::Plan && !t.plan_mode {
            return Decision::Deny(format!("{PLAN_BLOCKED_PREFIX} 计划模式下不能执行 {}（只允许探查类操作：导入/查看结构/只读 SQL/加载 skill）。请继续探查，然后调用 exit_plan_mode 提交计划。Plan mode: {} is not available; finish exploring and call exit_plan_mode.", t.name, t.name));
        }
        if mode == Mode::Plan && t.risk == Risk::External {
            return Decision::Deny(format!("{PLAN_BLOCKED_PREFIX} 计划模式下不能执行会影响外部的操作（{}）。请先提交计划，批准后再执行。Plan mode: external actions are not allowed.", t.summary));
        }
        if mode == Mode::Plan && t.plan_mode && matches!(t.risk, Risk::ReadOnly | Risk::Sandboxed) { return Decision::Allow; }
        if hit(Behavior::Ask).is_some() && mode != Mode::Full { return Decision::Ask; }
        if hit(Behavior::Allow).is_some() || self.session_allows.lock().unwrap().iter().any(|r| r.matches(t.name, args, t.summary)) { return Decision::Allow; }
        match (t.risk, mode) {
            (Risk::ReadOnly, _) | (_, Mode::Full) => Decision::Allow,
            (Risk::Sandboxed, Mode::Ask) | (Risk::Sandboxed, Mode::Auto) => Decision::Allow,
            (Risk::Network, Mode::Auto) => Decision::Allow,
            (Risk::Network, _) | (Risk::External, _) => Decision::Ask,
            (Risk::Sandboxed, Mode::Plan) => Decision::Deny("not allowed in plan mode".into()),
        }
    }

    /// Records an approval; returns a rule to persist for AllowAlways.
    pub fn record(&self, choice: Choice, tool: &str, args: &Value) -> Option<Rule> {
        let r = Rule { tool: tool.into(), pattern: suggest(args), behavior: Behavior::Allow };
        match choice {
            Choice::AllowSession => { self.session_allows.lock().unwrap().push(r); None }
            Choice::AllowAlways => {
                let mut rules = self.rules.lock().unwrap();
                if !rules.contains(&r) { rules.push(r.clone()); }
                Some(r)
            }
            _ => None,
        }
    }
}

/// "Always allow": append the rule to config.json `rules` (same shape as the Swift `PermissionRule`), keeping every
/// other key as is. Atomic write; no-op when the rule is already there.
pub fn persist_rule(config_path: &std::path::Path, r: &Rule) -> Result<bool, String> {
    let mut v: Value = std::fs::read_to_string(config_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    let Some(o) = v.as_object_mut() else { return Err("config.json is not an object".into()) };
    let rv = serde_json::to_value(r).map_err(|e| e.to_string())?;
    let rules = o.entry("rules").or_insert_with(|| Value::Array(vec![]));
    if !rules.is_array() { *rules = Value::Array(vec![]); }
    let arr = rules.as_array_mut().unwrap();
    let same = |x: &Value| serde_json::from_value::<Rule>(x.clone()).map(|x| &x == r).unwrap_or(false);
    if arr.iter().any(same) { return Ok(false); }
    arr.push(rv);
    let tmp = config_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, config_path).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persist_rule_keeps_other_keys() {
        let d = std::env::temp_dir().join(format!("dp-rule-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.json");
        std::fs::write(&p, r#"{"providers":[{"id":"x"}],"rules":[{"tool":"python","behavior":"deny"}]}"#).unwrap();
        let r = Rule { tool: "web_fetch".into(), pattern: Some("domain:example.com".into()), behavior: Behavior::Allow };
        assert!(persist_rule(&p, &r).unwrap());
        assert!(!persist_rule(&p, &r).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["providers"][0]["id"], "x");
        assert_eq!(v["rules"].as_array().unwrap().len(), 2);
        assert_eq!(v["rules"][1]["pattern"], "domain:example.com");
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn matrix_basics() {
        let e = Engine::new(Mode::Plan, vec![]);
        let v = |n, r, p| ToolView { name: n, risk: r, plan_mode: p, summary: "" };
        assert!(matches!(e.decide(&v("python", Risk::Sandboxed, false), &json!({})), Decision::Deny(s) if s.starts_with(PLAN_BLOCKED_PREFIX)));
        assert_eq!(e.decide(&v("data_import", Risk::Sandboxed, true), &json!({})), Decision::Allow);
        e.set_mode(Mode::Ask);
        assert_eq!(e.decide(&v("web_fetch", Risk::Network, true), &json!({})), Decision::Ask);
        assert_eq!(e.decide(&v("python", Risk::Sandboxed, false), &json!({})), Decision::Allow);
        e.set_mode(Mode::Auto);
        assert_eq!(e.decide(&v("web_fetch", Risk::Network, true), &json!({})), Decision::Allow);
        assert_eq!(e.decide(&v("x", Risk::External, false), &json!({})), Decision::Ask);
        e.rules.lock().unwrap().push(Rule { tool: "web_fetch".into(), pattern: Some("domain:example.com".into()), behavior: Behavior::Deny });
        assert!(matches!(e.decide(&v("web_fetch", Risk::Network, true), &json!({"url": "https://a.example.com/x"})), Decision::Deny(_)));
    }
}
