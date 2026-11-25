//! Core data types. JSON layout of `ChatMessage` matches the Swift core's transcript records (camelCase,
//! `createdAt` = seconds since 2001-01-01) so either core can read the other's transcripts.
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::text;

pub const REF_EPOCH: f64 = 978_307_200.0; // 2001-01-01 (Swift Date reference)

fn new_id() -> String { text::short_id(12) }
fn now_ref() -> f64 { text::now() - REF_EPOCH }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role { System, User, Assistant, Tool }

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self { Role::System => "system", Role::User => "user", Role::Assistant => "assistant", Role::Tool => "tool" }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall { pub id: String, pub name: String, pub arguments: String }

impl ToolCall {
    pub fn parsed(&self) -> Option<Value> {
        let s = if self.arguments.trim().is_empty() { "{}" } else { self.arguments.as_str() };
        serde_json::from_str(s).ok()
    }
    pub fn wire(&self) -> Value { json!({"id": self.id, "name": self.name, "arguments": self.arguments}) }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImagePart { #[serde(rename = "dataURL")] pub data_url: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[serde(default = "new_id")] pub id: String,
    pub role: Role,
    #[serde(default)] pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub images: Option<Vec<ImagePart>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub turn_id: Option<String>,
    #[serde(default)] pub is_meta: bool,
    #[serde(default = "now_ref")] pub created_at: f64,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        ChatMessage { id: new_id(), role, content: content.into(), reasoning: None, tool_calls: None, tool_call_id: None,
                      name: None, images: None, turn_id: None, is_meta: false, created_at: now_ref() }
    }
    pub fn system(s: impl Into<String>) -> Self { Self::new(Role::System, s) }
    pub fn user(s: impl Into<String>) -> Self { Self::new(Role::User, s) }
    pub fn meta(s: impl Into<String>, turn: &str) -> Self {
        let mut m = Self::new(Role::User, s); m.is_meta = true; m.turn_id = Some(turn.into()); m
    }
    pub fn tool(content: impl Into<String>, call_id: &str, name: &str, turn: &str) -> Self {
        let mut m = Self::new(Role::Tool, content);
        m.tool_call_id = Some(call_id.into()); m.name = Some(name.into()); m.turn_id = Some(turn.into()); m
    }
    /// protocol/agent-event.schema.json `$defs/message`
    pub fn wire(&self) -> Value {
        let mut o = Map::new();
        o.insert("id".into(), json!(self.id));
        o.insert("role".into(), json!(self.role.as_str()));
        o.insert("content".into(), json!(self.content));
        o.insert("is_meta".into(), json!(self.is_meta));
        if let Some(r) = &self.reasoning { o.insert("reasoning".into(), json!(r)); }
        if let Some(t) = &self.tool_calls { o.insert("tool_calls".into(), Value::Array(t.iter().map(|c| c.wire()).collect())); }
        if let Some(t) = &self.tool_call_id { o.insert("tool_call_id".into(), json!(t)); }
        if let Some(n) = &self.name { o.insert("name".into(), json!(n)); }
        if let Some(t) = &self.turn_id { o.insert("turn_id".into(), json!(t)); }
        if let Some(i) = &self.images { if !i.is_empty() { o.insert("image_count".into(), json!(i.len())); } }
        Value::Object(o)
    }
}

#[derive(Clone, Debug)]
pub struct ToolSpec { pub name: String, pub description: String, pub parameters: Value }

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage { #[serde(default)] pub prompt_tokens: usize, #[serde(default)] pub completion_tokens: usize }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub tool_call_mode: String,          // native | hermes
    pub supports_parallel_tool_calls: bool,
    pub send_parallel_tool_calls_param: bool,
    pub thinking_style: String,          // none | qwenChatTemplate | deepseekChatTemplate | enableThinkingTop | thinkingObject
    pub thinking_enabled: bool,
    pub reasoning_replay: String,        // never | currentTurn
    pub supports_vision: bool,
    /// computer tool coordinates: relative1000 (Qwen3-VL, 0–1000) | absolutePixels (Qwen2.5-VL, screenshot pixels)
    pub coordinate_mode: String,
    pub temperature: Option<f64>,
    pub extra_body: Option<Value>,
    /// where `context_window` came from: manual | server (/models) | learned (error message) | table (built-in model
    /// table) | preset (model family guess) | default. Empty in configs written before the model table.
    pub context_source: String,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities { context_window: 32_768, max_output_tokens: 4096, tool_call_mode: "native".into(), supports_parallel_tool_calls: false,
                       send_parallel_tool_calls_param: false, thinking_style: "none".into(), thinking_enabled: false,
                       reasoning_replay: "never".into(), supports_vision: false, coordinate_mode: "relative1000".into(), temperature: None, extra_body: None,
                       context_source: String::new() }
    }
}

impl Capabilities {
    /// Same presets as the Swift core (`ModelCapabilities.preset`).
    pub fn preset(model: &str) -> Self {
        let mut c = Capabilities::default();
        let m = model.to_lowercase();
        if m.contains("qwen3") || m.contains("qwq") {
            c.context_window = 131_072; c.max_output_tokens = 8192; c.thinking_style = "qwenChatTemplate".into(); c.supports_parallel_tool_calls = true;
        } else if m.contains("qwen") {
            c.context_window = 32_768; c.max_output_tokens = 8192; c.supports_parallel_tool_calls = true;
        }
        if m.contains("deepseek") {
            c.context_window = 131_072; c.max_output_tokens = 8192;
            if m.contains("reasoner") || m.contains("-r1") || m.ends_with("r1") {
                c.reasoning_replay = "never".into();
                if m.contains("r1") { c.tool_call_mode = "hermes".into(); }
            } else {
                c.reasoning_replay = "currentTurn".into(); c.thinking_style = "thinkingObject".into();
            }
        }
        if m.contains("vl") || m.contains("vision") || m.contains("gpt-4o") { c.supports_vision = true; }
        if m.contains("qwen2.5-vl") { c.coordinate_mode = "absolutePixels".into(); }
        if ["qwen3.5", "qwen3.6", "qwen3.7", "qwen3.8", "qwen3.9", "qwen4", "deepseek-v4"].iter().any(|k| m.contains(k)) { c.supports_vision = true; }
        c.context_source = if c.context_window != 32_768 { "preset" } else { "default" }.into();
        // built-in model table wins over the family guess; output stays moderate (New API pre-charges quota by
        // max_tokens, and some upstreams reject values above their own cap)
        if let Some((ctx, out)) = crate::model_table::lookup(model) {
            c.context_window = ctx;
            c.max_output_tokens = out.unwrap_or(8192).clamp(1024, 16_384).min((ctx / 4).max(1024));
            c.context_source = "table".into();
        }
        c
    }

    /// Context window the presets gave before the model table existed (configs saved then carry it without a source).
    fn legacy_window(model: &str) -> usize {
        let m = model.to_lowercase();
        if m.contains("qwen3") || m.contains("qwq") || m.contains("deepseek") { 131_072 } else { 32_768 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider_id: String,
    pub model: String,
    #[serde(default)] pub capabilities: Option<Capabilities>,
}

impl ModelRef {
    /// Saved capabilities, else the preset. Saved ones from before the model table whose window is just the old
    /// preset value (not typed by the user) take the table's window.
    pub fn caps(&self) -> Capabilities {
        let Some(c) = &self.capabilities else { return Capabilities::preset(&self.model) };
        let mut c = c.clone();
        if c.context_source.is_empty() {
            if c.context_window == Capabilities::legacy_window(&self.model) {
                if let Some((ctx, _)) = crate::model_table::lookup(&self.model) { c.context_window = ctx; c.context_source = "table".into(); }
            }
            if c.context_source.is_empty() { c.context_source = "manual".into(); }
        }
        c
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(rename = "baseURL")] pub base_url: String,
    #[serde(default)] pub use_proxy: bool,
    #[serde(default, rename = "proxyURL")] pub proxy_url: Option<String>,
    #[serde(default)] pub proxy_mode: Option<String>,
    #[serde(default)] pub extra_headers: std::collections::BTreeMap<String, String>,
}

impl Provider {
    pub fn effective_proxy_mode(&self) -> String {
        self.proxy_mode.clone().unwrap_or_else(|| if self.use_proxy { "custom".into() } else { "none".into() })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Roles {
    #[serde(default)] pub main: Option<ModelRef>,
    #[serde(default)] pub fast: Option<ModelRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub max_turns: usize,
    pub compact_threshold: f64,
    pub tool_output_max_chars: usize,
    pub small_row_threshold: usize,
    pub memory_enabled: bool,
    pub response_language: String,
    pub proxy_mode: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub default_mode: String,
    pub local_only: bool,
    pub subagent_max_concurrent: usize,
    /// browser tool: empty = every domain allowed (same key as the Swift settings).
    pub browser_allowed_domains: Vec<String>,
    /// MCP servers (same key / shape as the Swift settings).
    pub mcp_servers: Vec<crate::mcp::ServerConfig>,
    /// computer tool (off by default) and its per-turn cap on state-changing actions — same keys as the Swift settings.
    pub computer_use_enabled: bool,
    pub computer_max_steps_per_turn: usize,
    /// idle kernels are stopped after this many seconds (same key as the Swift settings)
    pub kernel_idle_seconds: f64,
    /// read-only skill folders of other agents (Codex / Claude Code) — same keys as the Swift settings
    pub external_skills_enabled: bool,
    pub external_skill_dirs: Vec<String>,
    /// absolute auto-compaction cap in tokens (0 = only the window ratio); the smaller of the two wins (Codex `auto_compact_token_limit`)
    pub compact_token_limit: usize,
    /// who writes the compaction summary: "fast" (fast model, default) | "main"
    pub compact_model: String,
    /// document parsing (OCR) of scanned PDF pages and images (MinerU server)
    pub doc_parse: DocParse,
    /// weekly automatic backup to AppSupport/backups (keeps 4, no project data)
    pub auto_backup: bool,
}

/// `settings.docParse`: MinerU server for scanned pages / images. Primary tier (default advanced: no invented text,
/// reads seals) plus an optional cross-check tier (default basic: keeps lines under seals). API key (optional) lives in
/// the keychain as api_key "docparse".
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocParse {
    pub enabled: bool,
    pub server: String,
    pub primary_tier: String,
    pub cross_check: bool,
    pub check_tier: String,
}

impl Default for DocParse {
    fn default() -> Self {
        DocParse { enabled: true, server: String::new(), primary_tier: "advanced".into(), cross_check: true, check_tier: "basic".into() }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings { max_turns: 120, compact_threshold: 0.80, tool_output_max_chars: 8000, small_row_threshold: 5000, memory_enabled: true,
                   response_language: "auto".into(), proxy_mode: "system".into(), proxy_host: String::new(), proxy_port: 0,
                   default_mode: "ask".into(), local_only: false, subagent_max_concurrent: 4, browser_allowed_domains: vec![], mcp_servers: vec![], computer_use_enabled: false, computer_max_steps_per_turn: 40, kernel_idle_seconds: 900.0,
                   external_skills_enabled: true, external_skill_dirs: ["~/.codex/skills", "~/.codex/skills/.system", "~/.claude/skills"].map(String::from).to_vec(),
                   compact_token_limit: 0, compact_model: "fast".into(), doc_parse: DocParse::default(), auto_backup: true }
    }
}

/// The app's config.json (same file the Swift app writes; unknown keys are ignored).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)] pub providers: Vec<Provider>,
    #[serde(default)] pub roles: Roles,
    #[serde(default)] pub settings: Settings,
    #[serde(default)] pub rules: Vec<crate::permissions::Rule>,
}

pub fn s(v: &Value, k: &str) -> Option<String> { v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string()) }
pub fn i(v: &Value, k: &str) -> i64 { v.get(k).and_then(|x| x.as_f64()).map(|x| x as i64).unwrap_or(0) }
pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] { v.get(k).and_then(|x| x.as_array()).map(|x| x.as_slice()).unwrap_or(&[]) }
