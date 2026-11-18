//! OpenAI-compatible Chat Completions client (port of the Swift `OpenAICompatibleClient`): request body with
//! thinking-param styles, reasoning replay, hermes prompt-based tools, SSE streaming, `<think>` splitting, retries.
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::types::{Capabilities, ChatMessage, ModelRef, Provider, Role, ToolCall, ToolSpec, Usage};

#[derive(Debug)]
pub enum LlmError { Http(u16, String), ContextTooLong(String), Bad(String), Net(String), Cancelled }

/// Context length a server states in an error body ("maximum context length is 32768 tokens", "max_model_len 32768"…).
pub fn context_limit_from_error(body: &str) -> Option<usize> {
    let re = regex::Regex::new(r"(?i)(?:maximum context length|max_model_len|max_context_length|context length|context window|context_length)\D{0,24}?(\d{4,7})").ok()?;
    re.captures(body).and_then(|c| c[1].parse::<usize>().ok()).filter(|&n| n >= 2048)
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LlmError::Http(c, b) => {
                // gateways answer timeouts with an empty body or an nginx HTML page: say what it means instead
                let t = b.trim();
                if t.is_empty() || t.starts_with('<') {
                    let why = match c {
                        504 => "网关超时：模型在网关时限内没有开始回复（输入太长或服务繁忙）/ gateway timeout",
                        502 => "网关错误：模型服务不可用 / bad gateway",
                        503 => "服务暂不可用（繁忙或维护中）/ service unavailable",
                        429 => "请求过于频繁，被限流 / rate limited",
                        _ => "模型服务返回错误 / server error",
                    };
                    write!(f, "HTTP {c}（{why}）")
                } else { write!(f, "HTTP {c}: {}", crate::text::prefix(b, 600)) }
            }
            LlmError::ContextTooLong(b) => write!(f, "context too long: {}", crate::text::prefix(b, 300)),
            LlmError::Bad(s) => write!(f, "bad response: {s}"),
            LlmError::Net(s) => write!(f, "network error: {s}"),
            LlmError::Cancelled => write!(f, "cancelled"),
        }
    }
}

pub enum Delta { Content(String), Reasoning(String), ToolCallStarted(usize, String) }

#[derive(Debug, Clone, Default)]
pub struct Completion { pub content: String, pub reasoning: String, pub tool_calls: Vec<ToolCall>, pub finish: Option<String>, pub usage: Option<Usage> }

/// `agent`: one-shot completions (may think silently for minutes). `stream_agent`: SSE — tokens (incl. reasoning) keep
/// arriving, so a long silence means a stalled gateway: fail after STREAM_IDLE and retry instead of hanging 5 min per try.
#[derive(Clone)]
pub struct Client { pub provider: Provider, api_key: Option<String>, agent: ureq::Agent, stream_agent: ureq::Agent, pub max_retries: u32 }

pub const STREAM_IDLE: Duration = Duration::from_secs(90);

/// shExpMatch glob (`*`, `?`), case-insensitive.
pub fn sh_exp_match(host: &str, pat: &str) -> bool {
    let re = format!("^{}$", regex::escape(&pat.to_lowercase()).replace("\\*", ".*").replace("\\?", "."));
    regex::Regex::new(&re).map(|r| r.is_match(&host.to_lowercase())).unwrap_or(false)
}

/// Minimal PAC evaluation: `if (shExpMatch(host,"…") || …) return "…"; else if … else return "…";` and
/// dnsDomainIs / host == "…". Returns Some(Some(proxy)) / Some(None)=DIRECT, or None when the PAC is not understood.
pub fn eval_pac(pac: &str, host: &str) -> Option<Option<String>> {
    let ret = regex::Regex::new(r#"return\s+"([^"]+)"\s*;"#).ok()?;
    let sh = regex::Regex::new(r#"shExpMatch\(\s*host\s*,\s*"([^"]+)"\s*\)"#).ok()?;
    let dns = regex::Regex::new(r#"dnsDomainIs\(\s*host\s*,\s*"([^"]+)"\s*\)"#).ok()?;
    let eq = regex::Regex::new(r#"host\s*==\s*"([^"]+)""#).ok()?;
    let body = pac.split_once("FindProxyForURL")?.1;
    let mut last = 0;
    for m in ret.captures_iter(body) {
        let whole = m.get(0)?;
        let cond = &body[last..whole.start()];
        last = whole.end();
        let has_cond = cond.contains("if");
        let hit = !has_cond
            || sh.captures_iter(cond).any(|c| sh_exp_match(host, &c[1]))
            || dns.captures_iter(cond).any(|c| { let d = c[1].trim_start_matches('.').to_lowercase(); let h = host.to_lowercase(); h == d || h.ends_with(&format!(".{d}")) })
            || eq.captures_iter(cond).any(|c| c[1].eq_ignore_ascii_case(host));
        if !hit { continue; }
        let first = m[1].split(';').next().unwrap_or("").trim().to_string();
        return Some(match first.split_once(' ') {
            Some((kind, addr)) if kind.eq_ignore_ascii_case("PROXY") || kind.eq_ignore_ascii_case("HTTPS") => Some(format!("http://{}", addr.trim())),
            _ => None,
        });
    }
    None
}

/// System proxy for `url` (macOS settings via `scutil --proxy`): PAC first (like URLSession), then the HTTPS proxy,
/// honouring the exceptions list.
/// System proxy for `url` (platform specific: scutil + PAC on macOS).
pub fn system_proxy_for(url: &str) -> Option<String> { crate::platform::system_proxy_for(url) }

/// Proxy for a URL from the app settings (custom / system incl. PAC / none). Local addresses never use a proxy.
pub fn proxy_for(url: &str, s: &crate::types::Settings) -> Option<String> {
    if is_local(url) { return None; }
    match s.proxy_mode.as_str() {
        "custom" if !s.proxy_host.is_empty() && s.proxy_port > 0 => Some(format!("http://{}:{}", s.proxy_host, s.proxy_port)),
        "system" => system_proxy_for(url),
        _ => None,
    }
}

/// HTTP agent honouring the app's proxy settings; returns the proxy used (for messages).
pub fn http_agent(url: &str, s: &crate::types::Settings, secs: u64) -> Result<(ureq::Agent, Option<String>), String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let mut b = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).timeout_connect(Duration::from_secs(secs.min(30))).timeout_read(Duration::from_secs(secs));
    let proxy = proxy_for(url, s);
    if let Some(p) = &proxy { b = b.proxy(ureq::Proxy::new(p).map_err(|e| format!("proxy {p}: {e}"))?); }
    Ok((b.build(), proxy))
}

fn is_local(url: &str) -> bool {
    let h = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or("");
    h == "localhost" || h == "127.0.0.1" || h == "::1" || h.starts_with("10.") || h.starts_with("192.168.")
}

impl Client {
    pub fn new(provider: Provider, api_key: Option<String>, global_proxy: Option<String>) -> Result<Self, String> {
        let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
        let tls = Arc::new(tls);
        let mut b = ureq::AgentBuilder::new().tls_connector(tls.clone()).timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(300));
        let mut sb = ureq::AgentBuilder::new().tls_connector(tls).timeout_connect(Duration::from_secs(30)).timeout_read(STREAM_IDLE);
        let proxy = match provider.effective_proxy_mode().as_str() {
            "custom" => provider.proxy_url.clone().filter(|p| !p.is_empty()).map(|p| if p.contains("://") { p } else { format!("http://{p}") }),
            "none" => None,
            _ => match global_proxy.as_deref() {
                Some("system") => system_proxy_for(&provider.base_url),
                _ => global_proxy,
            },
        };
        if let Some(p) = proxy.filter(|_| !is_local(&provider.base_url)) {
            let px = ureq::Proxy::new(&p).map_err(|e| format!("proxy {p}: {e}"))?;
            b = b.proxy(px.clone());
            sb = sb.proxy(px);
        }
        Ok(Client { provider, api_key, agent: b.build(), stream_agent: sb.build(), max_retries: 3 })
    }

    pub fn endpoint(&self, path: &str) -> String {
        let mut base = self.provider.base_url.trim().trim_end_matches('/').to_string();
        if !base.ends_with("/v1") && !base.contains("/v1/") && !base.ends_with("/compatible-mode/v1") { base += "/v1"; }
        base + path
    }

    fn post(&self, body: &Value) -> Result<ureq::Response, LlmError> {
        let agent = if body["stream"] == true { &self.stream_agent } else { &self.agent };
        let mut r = agent.post(&self.endpoint("/chat/completions")).set("Content-Type", "application/json");
        if let Some(k) = self.api_key.as_deref().filter(|k| !k.is_empty()) { r = r.set("Authorization", &format!("Bearer {k}")); }
        for (k, v) in &self.provider.extra_headers { r = r.set(k, v); }
        match r.send_string(&body.to_string()) {
            Ok(resp) => Ok(resp),
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                let low = text.to_lowercase();
                if code == 400 && (low.contains("context") || low.contains("too long") || (low.contains("maximum") && low.contains("token"))) {
                    return Err(LlmError::ContextTooLong(text));
                }
                Err(LlmError::Http(code, text))
            }
            Err(e) => Err(LlmError::Net(e.to_string())),
        }
    }

    fn with_retry<T>(&self, cancel: &AtomicBool, mut f: impl FnMut() -> Result<T, LlmError>) -> Result<T, LlmError> {
        let mut attempt = 0;
        loop {
            let r = f();
            let retry = matches!(&r, Err(LlmError::Http(c, _)) if [408, 429, 500, 502, 503, 504].contains(c)) || matches!(&r, Err(LlmError::Net(_)));
            if retry && attempt < self.max_retries && !cancel.load(Ordering::SeqCst) {
                attempt += 1;
                // back off, but wake up at once when the user presses Stop
                let until = std::time::Instant::now() + Duration::from_secs(1 << attempt);
                while std::time::Instant::now() < until {
                    if cancel.load(Ordering::SeqCst) { return Err(LlmError::Cancelled); }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
            return r;
        }
    }

    pub fn stream(&self, model: &ModelRef, messages: &[ChatMessage], tools: &[ToolSpec], cancel: &AtomicBool,
                  on_delta: &mut dyn FnMut(Delta)) -> Result<Completion, LlmError> {
        let caps = model.caps();
        let body = build_body(&model.model, &caps, messages, tools, true);
        // The blocking HTTP request and the SSE reads run on a helper thread; this thread only waits on a channel and
        // checks `cancel` every 100 ms, so Stop takes effect at once even while the gateway is slow to answer or silent.
        // An abandoned helper ends on its own (its sends fail once we return; reads time out after STREAM_IDLE).
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, LlmError>>();
        let (me, flag) = (self.clone(), Arc::new(AtomicBool::new(false)));
        let f2 = flag.clone();
        std::thread::spawn(move || {
            let resp = match me.with_retry(&f2, || me.post(&body)) { Ok(r) => r, Err(e) => { let _ = tx.send(Err(e)); return; } };
            for line in BufReader::new(resp.into_reader()).lines() {
                let r = line.map_err(|e| LlmError::Net(e.to_string()));
                let stop = r.is_err();
                if tx.send(r).is_err() || stop || f2.load(Ordering::SeqCst) { return; }
            }
        });
        let mut p = StreamProcessor::new(caps.tool_call_mode == "hermes");
        loop {
            if cancel.load(Ordering::SeqCst) { flag.store(true, Ordering::SeqCst); return Err(LlmError::Cancelled); }
            let line = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(l) => l?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let l = line.trim_end_matches('\r');
            let data = if let Some(d) = l.strip_prefix("data:") { d.trim_start() } else if l.starts_with('{') { l } else { continue };
            if data == "[DONE]" { break; }
            p.handle(data, on_delta)?;
        }
        Ok(p.result())
    }

    /// `GET /models` → sorted model ids (OpenAI-compatible list).
    pub fn list_models(&self) -> Result<Vec<String>, LlmError> { Ok(self.list_models_meta()?.into_iter().map(|x| x.0).collect()) }

    /// Model ids with the context length the server reports, if any (vLLM `max_model_len`, LM Studio / OpenRouter
    /// `context_length`, …). New API gateways usually report none.
    pub fn list_models_meta(&self) -> Result<Vec<(String, Option<usize>)>, LlmError> {
        let mut r = self.agent.get(&self.endpoint("/models"));
        if let Some(k) = self.api_key.as_deref().filter(|k| !k.is_empty()) { r = r.set("Authorization", &format!("Bearer {k}")); }
        for (k, v) in &self.provider.extra_headers { r = r.set(k, v); }
        let text = match r.call() {
            Ok(resp) => resp.into_string().map_err(|e| LlmError::Net(e.to_string()))?,
            Err(ureq::Error::Status(code, resp)) => return Err(LlmError::Http(code, resp.into_string().unwrap_or_default())),
            Err(e) => return Err(LlmError::Net(e.to_string())),
        };
        let v: Value = serde_json::from_str(&text).map_err(|e| LlmError::Bad(e.to_string()))?;
        let ctx = |m: &Value| ["max_model_len", "context_length", "context_window", "max_context_length", "max_input_tokens"].iter()
            .find_map(|k| m[*k].as_u64()).or_else(|| m["top_provider"]["context_length"].as_u64()).map(|n| n as usize).filter(|&n| n >= 2048);
        let mut ids: Vec<(String, Option<usize>)> = v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(|id| (id.to_string(), ctx(m)))).collect()).unwrap_or_default();
        ids.sort();
        Ok(ids)
    }

    /// One-shot completion, no tools. Streamed under the hood: gateways (nginx `proxy_read_timeout`, often 60 s) cut a
    /// non-streaming request whose whole answer takes longer than that (HTTP 504); with SSE only the first token has to
    /// arrive in time. A 504 during a long prefill is retried — the server has usually cached the prefix by then.
    pub fn complete(&self, model: &ModelRef, messages: &[ChatMessage]) -> Result<Completion, LlmError> {
        let never = AtomicBool::new(false);
        let mut r = self.stream(model, messages, &[], &never, &mut |_| {})?;
        // always-thinking models (glm-5.x: thinking cannot be switched off) may spend the whole output budget on
        // reasoning and return no answer: retry once with room to finish thinking and answer
        if out_of_budget(&r) {
            let mut m = model.clone();
            let mut caps = m.caps();
            let used = caps.max_output_tokens;
            caps.max_output_tokens = (used * 4).max(16_384).min((caps.context_window / 2).max(used));
            if caps.max_output_tokens > used {
                m.capabilities = Some(caps);
                r = self.stream(&m, messages, &[], &never, &mut |_| {})?;
            }
        }
        r.tool_calls.clear();
        Ok(r)
    }
}

/// The output limit was hit before any answer text (all of it went to reasoning).
pub fn out_of_budget(r: &Completion) -> bool { r.finish.as_deref() == Some("length") && r.content.trim().is_empty() }

/// Request body — same fields and rules as the Swift `buildBody`.
pub fn build_body(model: &str, caps: &Capabilities, messages: &[ChatMessage], tools: &[ToolSpec], stream: bool) -> Value {
    let hermes = caps.tool_call_mode == "hermes";
    let mut sys_prefix = if hermes && !tools.is_empty() { hermes_system_prompt(tools) } else { String::new() };
    let last_user = messages.iter().rposition(|m| m.role == Role::User && !m.is_meta).map(|i| i as isize).unwrap_or(-1);
    let mut msgs: Vec<Value> = vec![];
    for (i, m) in messages.iter().enumerate() {
        match m.role {
            Role::System => {
                let c = if sys_prefix.is_empty() { m.content.clone() } else { format!("{}\n\n{}", m.content, sys_prefix) };
                sys_prefix.clear();
                msgs.push(json!({"role": "system", "content": c}));
            }
            Role::User => match &m.images {
                Some(imgs) if !imgs.is_empty() && caps.supports_vision => {
                    let mut parts = vec![json!({"type": "text", "text": m.content})];
                    parts.extend(imgs.iter().map(|im| json!({"type": "image_url", "image_url": {"url": im.data_url}})));
                    msgs.push(json!({"role": "user", "content": parts}));
                }
                _ => msgs.push(json!({"role": "user", "content": m.content})),
            },
            Role::Assistant => {
                let mut d = Map::new();
                d.insert("role".into(), json!("assistant"));
                if hermes {
                    let mut t = m.content.clone();
                    for c in m.tool_calls.iter().flatten() {
                        t += &format!("\n<tool_call>\n{}\n</tool_call>", json!({"name": c.name, "arguments": c.parsed().unwrap_or(json!({}))}));
                    }
                    d.insert("content".into(), json!(t));
                } else {
                    d.insert("content".into(), json!(m.content));
                    if let Some(tc) = m.tool_calls.as_ref().filter(|t| !t.is_empty()) {
                        d.insert("tool_calls".into(), Value::Array(tc.iter().map(|c| json!({"id": c.id, "type": "function",
                            "function": {"name": c.name, "arguments": if c.arguments.is_empty() { "{}" } else { c.arguments.as_str() }}})).collect()));
                    }
                }
                if caps.reasoning_replay == "currentTurn" && (i as isize) > last_user {
                    if let Some(r) = m.reasoning.as_ref().filter(|r| !r.is_empty()) { d.insert("reasoning_content".into(), json!(r)); }
                }
                msgs.push(Value::Object(d));
            }
            Role::Tool => {
                if hermes {
                    let resp = format!("<tool_response>\n{}\n</tool_response>", json!({"name": m.name.clone().unwrap_or_else(|| "tool".into()), "content": m.content}));
                    let merged = match msgs.last_mut() {
                        Some(last) if last["role"] == "user" && last["content"].as_str().map(|c| c.starts_with("<tool_response>")).unwrap_or(false) => {
                            let c = format!("{}\n{}", last["content"].as_str().unwrap(), resp);
                            last["content"] = json!(c); true
                        }
                        _ => false,
                    };
                    if !merged { msgs.push(json!({"role": "user", "content": resp})); }
                } else {
                    msgs.push(json!({"role": "tool", "tool_call_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content}));
                }
                if let Some(imgs) = m.images.as_ref().filter(|i| !i.is_empty() && caps.supports_vision) {
                    let mut parts = vec![json!({"type": "text", "text": format!("[screenshot from {}]", m.name.clone().unwrap_or_else(|| "tool".into()))})];
                    parts.extend(imgs.iter().map(|im| json!({"type": "image_url", "image_url": {"url": im.data_url}})));
                    msgs.push(json!({"role": "user", "content": parts}));
                }
            }
        }
    }
    if !sys_prefix.is_empty() { msgs.insert(0, json!({"role": "system", "content": sys_prefix})); }
    let mut body = json!({"model": model, "messages": msgs, "stream": stream, "max_tokens": caps.max_output_tokens});
    if stream { body["stream_options"] = json!({"include_usage": true}); }
    if let Some(t) = caps.temperature { body["temperature"] = json!(t); }
    if !hermes && !tools.is_empty() {
        body["tools"] = Value::Array(tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})).collect());
        body["tool_choice"] = json!("auto");
        if caps.send_parallel_tool_calls_param { body["parallel_tool_calls"] = json!(caps.supports_parallel_tool_calls); }
    }
    match caps.thinking_style.as_str() {
        "qwenChatTemplate" => body["chat_template_kwargs"] = json!({"enable_thinking": caps.thinking_enabled}),
        "deepseekChatTemplate" => body["chat_template_kwargs"] = json!({"thinking": caps.thinking_enabled}),
        "enableThinkingTop" => body["enable_thinking"] = json!(caps.thinking_enabled),
        "thinkingObject" => body["thinking"] = json!({"type": if caps.thinking_enabled { "enabled" } else { "disabled" }}),
        _ => {}
    }
    if let Some(Value::Object(extra)) = &caps.extra_body { for (k, v) in extra { body[k] = v.clone(); } }
    body
}

pub struct StreamProcessor {
    content: String, reasoning: String, partials: BTreeMap<usize, (String, String, String)>,
    finish: Option<String>, usage: Option<Usage>, hermes: bool, in_think: bool, saw_content: bool, pending: String,
}

impl StreamProcessor {
    pub fn new(hermes: bool) -> Self {
        StreamProcessor { content: String::new(), reasoning: String::new(), partials: BTreeMap::new(), finish: None, usage: None,
                          hermes, in_think: false, saw_content: false, pending: String::new() }
    }

    pub fn handle(&mut self, data: &str, on: &mut dyn FnMut(Delta)) -> Result<(), LlmError> {
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(()) };
        if let Some(e) = v.get("error") {
            return Err(LlmError::Bad(e.get("message").and_then(|m| m.as_str()).map(String::from).unwrap_or_else(|| e.to_string())));
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(Usage { prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as usize, completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as usize });
        }
        let Some(choice) = v.pointer("/choices/0") else { return Ok(()) };
        if let Some(f) = choice.get("finish_reason").and_then(|f| f.as_str()) { self.finish = Some(f.into()); }
        let delta = choice.get("delta").or_else(|| choice.get("message")).cloned().unwrap_or(json!({}));
        if let Some(r) = delta.get("reasoning_content").or_else(|| delta.get("reasoning")).and_then(|r| r.as_str()).filter(|r| !r.is_empty()) {
            self.reasoning += r; on(Delta::Reasoning(r.into()));
        }
        if let Some(c) = delta.get("content").and_then(|c| c.as_str()).filter(|c| !c.is_empty()) { self.handle_content(c, on); }
        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for (pos, tc) in tcs.iter().enumerate() {
                let idx = tc.get("index").and_then(|i| i.as_u64()).map(|i| i as usize).unwrap_or(pos);
                let is_new = !self.partials.contains_key(&idx);
                let p = self.partials.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc.get("id").and_then(|i| i.as_str()).filter(|s| !s.is_empty()) { p.0 = id.into(); }
                let f = tc.get("function");
                if let Some(n) = f.and_then(|f| f.get("name")).and_then(|n| n.as_str()).filter(|s| !s.is_empty()) {
                    if p.1.is_empty() { p.1 = n.into(); }
                    if is_new { on(Delta::ToolCallStarted(idx, n.into())); }
                }
                if let Some(a) = f.and_then(|f| f.get("arguments")).and_then(|a| a.as_str()) { p.2 += a; }
            }
        }
        Ok(())
    }

    fn handle_content(&mut self, c: &str, on: &mut dyn FnMut(Delta)) {
        let mut s = std::mem::take(&mut self.pending) + c;
        while !s.is_empty() {
            if self.in_think {
                if let Some(i) = s.find("</think>") {
                    let t = s[..i].to_string();
                    if !t.is_empty() { self.reasoning += &t; on(Delta::Reasoning(t)); }
                    self.in_think = false; s = s[i + 8..].to_string();
                } else if let Some(k) = partial_suffix(&s, "</think>") {
                    let cut = s.len() - k;
                    let t = s[..cut].to_string();
                    if !t.is_empty() { self.reasoning += &t; on(Delta::Reasoning(t)); }
                    self.pending = s[cut..].to_string(); s.clear();
                } else { self.reasoning += &s; on(Delta::Reasoning(s.clone())); s.clear(); }
            } else {
                if !self.saw_content {
                    if let Some(i) = s.find("<think>") {
                        if s[..i].trim().is_empty() { self.in_think = true; s = s[i + 7..].to_string(); continue; }
                    }
                    if !s.trim().is_empty() && partial_suffix(&s, "<think>") == Some(s.len()) { self.pending = s; return; }
                }
                if !s.trim().is_empty() { self.saw_content = true; }
                self.content += &s;
                on(Delta::Content(s.clone()));
                s.clear();
            }
        }
    }

    pub fn result(self) -> Completion {
        let mut text = self.content + &self.pending;
        let mut calls: Vec<ToolCall> = self.partials.into_values().filter(|p| !p.1.is_empty()).map(|(id, name, args)| ToolCall {
            id: if id.is_empty() { format!("call_{}", crate::text::short_id(16)) } else { id }, name, arguments: repair_json(&args) }).collect();
        if self.hermes || (calls.is_empty() && text.contains("<tool_call>")) {
            let (t, c) = hermes_parse(&text); text = t; calls.extend(c);
        }
        Completion { content: text, reasoning: self.reasoning, tool_calls: calls, finish: self.finish, usage: self.usage }
    }
}

fn partial_suffix(s: &str, tag: &str) -> Option<usize> {
    (1..tag.len().min(s.len() + 1)).rev().find(|&k| s.is_char_boundary(s.len() - k) && tag.starts_with(&s[s.len() - k..]))
}

pub fn split_think(t: &str) -> (String, String) {
    match (t.find("<think>"), t.find("</think>")) {
        (Some(a), Some(b)) if a + 7 <= b => ((t[..a].to_string() + &t[b + 8..]).trim().to_string(), t[a + 7..b].trim().to_string()),
        _ => (t.to_string(), String::new()),
    }
}

/// Light repair for streaming artefacts: empty args, "{..}{..}", missing closing braces.
pub fn repair_json(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() { return "{}".into(); }
    if serde_json::from_str::<Value>(t).is_ok() { return t.into(); }
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, ch) in t.char_indices() {
        if in_str { if esc { esc = false } else if ch == '\\' { esc = true } else if ch == '"' { in_str = false } continue; }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => { depth -= 1; if depth == 0 { let c = &t[..=i]; if serde_json::from_str::<Value>(c).is_ok() { return c.into(); } } }
            _ => {}
        }
    }
    if depth > 0 { let c = format!("{t}{}", "}".repeat(depth as usize)); if serde_json::from_str::<Value>(&c).is_ok() { return c; } }
    t.into()
}

pub fn hermes_system_prompt(tools: &[ToolSpec]) -> String {
    let defs: Vec<String> = tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}).to_string()).collect();
    format!("# Tools\n\nYou may call one or more functions to assist with the user query.\nFunction signatures are provided within <tools></tools> XML tags:\n<tools>\n{}\n</tools>\n\nFor each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:\n<tool_call>\n{{\"name\": <function-name>, \"arguments\": <args-json-object>}}\n</tool_call>", defs.join("\n"))
}

pub fn hermes_parse(text: &str) -> (String, Vec<ToolCall>) {
    let (mut out, mut calls, mut rest) = (String::new(), vec![], text);
    while let Some(a) = rest.find("<tool_call>") {
        out += &rest[..a];
        let after = &rest[a + 11..];
        let Some(b) = after.find("</tool_call>") else { out += &rest[a..]; rest = ""; break };
        if let Ok(v) = serde_json::from_str::<Value>(&repair_json(after[..b].trim())) {
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                let args = match v.get("arguments") { Some(Value::String(s)) => s.clone(), Some(a) => a.to_string(), None => "{}".into() };
                calls.push(ToolCall { id: format!("call_{}", crate::text::short_id(16)), name: name.into(), arguments: args });
            }
        }
        rest = &after[b + 12..];
    }
    out += rest;
    (out.trim().to_string(), calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_think_and_tools() {
        let mut p = StreamProcessor::new(false);
        let mut seen = vec![];
        let mut on = |d: Delta| if let Delta::Content(s) = d { seen.push(s) };
        for d in [r#"{"choices":[{"delta":{"content":"<thi"}}]}"#, r#"{"choices":[{"delta":{"content":"nk>hmm</think>答案"}}]}"#,
                  r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"sql_query","arguments":"{\"sql\":"}}]}}]}"#,
                  r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"select 1\"}"}}]},"finish_reason":"tool_calls"}]}"#] {
            p.handle(d, &mut on).unwrap();
        }
        let r = p.result();
        assert_eq!(r.content, "答案"); assert_eq!(r.reasoning, "hmm");
        assert_eq!(context_limit_from_error(r#"{"error":{"message":"This model's maximum context length is 32768 tokens. However, you requested 40000 tokens"}}"#), Some(32768));
        assert_eq!(context_limit_from_error("max_model_len (65536) exceeded"), Some(65536));
        assert_eq!(context_limit_from_error("rate limited"), None);
        assert_eq!(r.tool_calls[0].arguments, "{\"sql\":\"select 1\"}");
        assert_eq!(repair_json("{\"a\":1}{\"b\":2}"), "{\"a\":1}");
        let (t, c) = hermes_parse("hi <tool_call>{\"name\":\"x\",\"arguments\":{\"a\":1}}</tool_call>");
        assert_eq!(t, "hi"); assert_eq!(c[0].name, "x");
        let pac = r#"function FindProxyForURL(url, host) { if ((shExpMatch(host, "*.google.com") || shExpMatch(host, "google.com"))) return "PROXY p1:3128"; else if ((shExpMatch(host, "*.cn"))) return "DIRECT"; else return "PROXY p2:8080; DIRECT"; }"#;
        assert_eq!(eval_pac(pac, "www.google.com"), Some(Some("http://p1:3128".into())));
        assert_eq!(eval_pac(pac, "aichat.dukekunshan.edu.cn"), Some(None));
        assert_eq!(eval_pac(pac, "example.org"), Some(Some("http://p2:8080".into())));
    }
}
