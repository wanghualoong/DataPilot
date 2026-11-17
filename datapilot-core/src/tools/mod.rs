//! Tool registry. Contracts (name, description, parameters, risk, flags) come from protocol/tools.snapshot.json —
//! the single declarative source shared with the Swift core (ADR-0012 rule 6); Rust only supplies `run` functions.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::Emit;
use crate::permissions::Risk;
use crate::session::Shared;
use crate::types::ToolSpec;

pub mod agent;
pub mod bi;
pub mod data;
pub mod db;
pub mod web;
pub mod browser;
#[cfg(target_os = "macos")]
pub mod computer;

#[derive(Clone, Debug)]
pub struct Contract { pub name: String, pub description: String, pub parameters: Value, pub risk: Risk, pub plan_mode: bool, pub concurrency_safe: bool }

#[derive(Clone, Debug, Default)]
pub struct ToolResult { pub content: String, pub is_error: bool, pub display: Option<Value>, pub images: Option<Vec<crate::types::ImagePart>> }

impl ToolResult {
    pub fn ok(s: impl Into<String>) -> Self { ToolResult { content: s.into(), ..Default::default() } }
    pub fn with(s: impl Into<String>, d: Value) -> Self { ToolResult { content: s.into(), display: Some(d), ..Default::default() } }
    pub fn err(s: impl Into<String>) -> Self { ToolResult { content: s.into(), is_error: true, ..Default::default() } }
}

pub struct ToolCtx<'a> {
    pub shared: &'a Arc<Shared>,
    pub turn_id: String,
    pub call_id: String,
    pub emit: Emit,
    /// Sub-agents with Python get their own namespace on the same kernel process.
    pub kernel_override: Option<crate::kernel::KernelClient>,
    /// Sub-agent types with `memory:` keep their own memory folder (memory_write scope "agent").
    pub agent_memory_dir: Option<PathBuf>,
}

impl ToolCtx<'_> {
    pub fn kernel_client(&self) -> &crate::kernel::KernelClient { self.kernel_override.as_ref().unwrap_or(&self.shared.kernel) }
    /// Kernel calls from tools stop waiting as soon as the user presses Stop.
    pub fn kernel(&self, op: &str, p: Value, timeout: u64) -> Result<Value, String> { self.kernel_full(op, p, timeout).map(|r| r.result) }
    pub fn kernel_full(&self, op: &str, p: Value, timeout: u64) -> Result<crate::kernel::KernelResponse, String> {
        self.kernel_client().call_cancellable(op, p, timeout, &self.shared.broker.cancel)
    }
    pub fn workspace(&self) -> &Path { &self.shared.workspace }
    pub fn read_roots(&self) -> Vec<PathBuf> { self.shared.project_root.iter().cloned().chain([self.shared.workspace.clone()]).collect() }
}

pub type RunFn = fn(&Value, &ToolCtx) -> Result<ToolResult, String>;
pub type SummaryFn = fn(&Value) -> String;
pub type RiskFn = fn(&Value) -> Risk;

pub struct Impl { pub run: RunFn, pub summary: Option<SummaryFn>, pub risk_for: Option<RiskFn> }

pub fn imp(run: RunFn, summary: SummaryFn) -> Impl { Impl { run, summary: Some(summary), risk_for: None } }
pub fn plain(run: RunFn) -> Impl { Impl { run, summary: None, risk_for: None } }
pub fn with_risk(mut i: Impl, f: RiskFn) -> Impl { i.risk_for = Some(f); i }

pub struct Tool { pub contract: Contract, imp: Impl, remote: Option<crate::mcp::Remote> }

impl Tool {
    /// An MCP server tool (`mcp__<server>__<tool>`).
    pub fn remote(contract: Contract, r: crate::mcp::Remote) -> Tool {
        fn unused(_: &Value, _: &ToolCtx) -> Result<ToolResult, String> { Err("not callable".into()) }
        Tool { contract, imp: plain(unused), remote: Some(r) }
    }
    pub fn name(&self) -> &str { &self.contract.name }
    pub fn risk_for(&self, a: &Value) -> Risk { self.imp.risk_for.map(|f| f(a)).unwrap_or(self.contract.risk) }
    pub fn needs_network(&self, a: &Value) -> bool {
        match &self.remote { Some(r) => r.client.cfg.transport == "http", None => self.risk_for(a) == Risk::Network }
    }
    pub fn summary(&self, a: &Value) -> String {
        match self.imp.summary { Some(f) => f(a), None => format!("{} {}", self.contract.name, crate::text::prefix(&a.to_string(), 120)) }
    }
    pub fn run(&self, a: &Value, ctx: &ToolCtx) -> Result<ToolResult, String> {
        match &self.remote { Some(r) => crate::mcp::call(r, a, ctx), None => (self.imp.run)(a, ctx) }
    }
    pub fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.contract.name.clone(), description: self.contract.description.clone(), parameters: self.contract.parameters.clone() }
    }
}

/// Same order as the Swift `DefaultTools.registry()` (stable prompt prefix). Tools not yet ported are skipped.
pub const ORDER: &[&str] = &[
    "data_import", "data_schema", "sql_query", "python", "data_transform", "data_export", "relation_test", "relation_define", "catalog_update",
    "db_list", "db_schema", "db_query", "db_extract", "db_refresh", "query_save",
    "chart_create", "dashboard_create", "dashboard_update",
    "memory_write", "memory_search", "memory_forget", "history_search",
    "todo_write", "ask_user", "exit_plan_mode", "skill", "save_skill",
    "list_files", "read_file", "grep", "glob", "write_file", "bash", "web_search", "web_fetch", "agent", "browser", "computer",
];

fn impls() -> HashMap<&'static str, Impl> {
    let mut m = HashMap::new();
    data::register(&mut m);
    bi::register(&mut m);
    db::register(&mut m);
    agent::register(&mut m);
    web::register(&mut m);
    browser::register(&mut m);
    #[cfg(target_os = "macos")]
    computer::register(&mut m);
    m
}

pub struct Registry { pub tools: Vec<Tool> }

impl Registry {
    pub fn load(snapshot: &Path) -> Result<Registry, String> {
        let text = std::fs::read_to_string(snapshot).map_err(|e| format!("{}: {e}", snapshot.display()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut contracts: HashMap<String, Contract> = HashMap::new();
        for c in v.as_array().ok_or("snapshot must be an array")? {
            let name = c["name"].as_str().unwrap_or("").to_string();
            contracts.insert(name.clone(), Contract {
                name, description: c["description"].as_str().unwrap_or("").to_string(), parameters: c["parameters"].clone(),
                risk: Risk::parse(c["risk"].as_str().unwrap_or("sandboxed")), plan_mode: c["plan_mode"].as_bool().unwrap_or(false),
                concurrency_safe: c["concurrency_safe"].as_bool().unwrap_or(false) });
        }
        let mut imps = impls();
        let mut tools = vec![];
        for n in ORDER {
            if let (Some(c), Some(i)) = (contracts.get(*n), imps.remove(n)) { tools.push(Tool { contract: c.clone(), imp: i, remote: None }); }
        }
        Ok(Registry { tools })
    }
    pub fn get(&self, name: &str) -> Option<&Tool> { self.tools.iter().find(|t| t.name() == name || (name == "subagent" && t.name() == "agent")) }
    pub fn names(&self) -> Vec<String> { self.tools.iter().map(|t| t.contract.name.clone()).collect() }
}

/// Minimal JSON-schema validation (required keys, primitive types, enums) — same rules as the Swift validator.
pub fn validate(args: &Value, schema: &Value) -> Option<String> {
    let Some(obj) = args.as_object() else { return Some("arguments must be a JSON object".into()) };
    for r in schema["required"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        if obj.get(r).map(|v| v.is_null()).unwrap_or(true) { return Some(format!("missing required parameter '{r}'")); }
    }
    let props = schema["properties"].as_object();
    for (k, v) in obj {
        let Some(p) = props.and_then(|p| p.get(k)) else { continue };
        let Some(t) = p["type"].as_str() else { continue };
        let ok = v.is_null() || match t {
            "string" => v.is_string(), "boolean" => v.is_boolean(), "number" => v.is_number(), "array" => v.is_array(), "object" => v.is_object(),
            "integer" => v.as_f64().map(|n| n.round() == n).unwrap_or(false), _ => false,
        };
        if !ok { return Some(format!("parameter '{k}' must be of type {t}")); }
        if let Some(en) = p["enum"].as_array() { if !en.contains(v) { return Some(format!("parameter '{k}' must be one of {}", Value::Array(en.clone()))); } }
    }
    None
}

/// Copy the listed keys (non-null) from the model's arguments into kernel params.
pub fn pick(a: &Value, keys: &[&str]) -> Value {
    let mut m = serde_json::Map::new();
    for k in keys { if let Some(v) = a.get(*k).filter(|v| !v.is_null()) { m.insert((*k).into(), v.clone()); } }
    Value::Object(m)
}

pub fn sv(a: &Value, k: &str) -> String { a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string() }

/// Markdown table for the model (same cell rules as the Swift `renderTable`).
pub fn render_table(columns: &[String], rows: &[Value], max_rows: usize) -> String {
    if columns.is_empty() { return "(no columns)".into(); }
    let cell = |v: &Value| -> String {
        match v {
            Value::Null => String::new(),
            Value::String(s) => crate::text::prefix(s, 60).replace('|', "\\|").replace('\n', " "),
            Value::Number(n) => crate::text::fmt_num(n.as_f64().unwrap_or(0.0)),
            Value::Bool(b) => b.to_string(),
            other => other.to_string(),
        }
    };
    let mut lines = vec![format!("| {} |", columns.join(" | ")), format!("|{}|", columns.iter().map(|_| "---").collect::<Vec<_>>().join("|"))];
    for r in rows.iter().take(max_rows) {
        lines.push(format!("| {} |", r.as_array().map(|a| a.iter().map(cell).collect::<Vec<_>>().join(" | ")).unwrap_or_default()));
    }
    lines.join("\n")
}

pub fn strs(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

pub fn display_of(kind: &str) -> Value { json!({"kind": kind}) }
