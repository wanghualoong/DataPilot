//! Dashboard tools (ports of Swift DashboardCreateTool / DashboardUpdateTool / DashboardValidator).
//! Specs live in <workspace>/dashboards/<id>.json — same files the app's dashboard panel reads.
//! calcFields / measures / params are pre-checked by running Resources/web/expr.js (the dashboard's own engine) in
//! QuickJS — one implementation for renderer, Swift (JavaScriptCore) and Rust.
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::{imp, plain, sv, Impl, ToolCtx, ToolResult};
use crate::text;
use crate::types::arr;

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("dashboard_create", imp(dashboard_create, |a| format!("看板：{}", sv(&a["spec"], "title"))));
    m.insert("dashboard_update", plain(dashboard_update));
}

const CHART_TYPES: &[&str] = &["bar", "hbar", "line", "area", "pie", "donut", "radar", "scatter", "heatmap", "funnel", "treemap", "table", "kpi", "stacked_bar"];
const AGGS: &[&str] = &["sum", "avg", "count", "max", "min", "count_distinct", "none"];
const REF_TYPES: &[&str] = &["avg", "median", "max", "min", "minmax", "value", "target"];
const CALCS: &[&str] = &["yoy", "pop", "cumsum", "share", "ma3"];

fn dir(ws: &Path) -> PathBuf { ws.join("dashboards") }
pub fn load(id: &str, ws: &Path) -> Option<Value> {
    std::fs::read_to_string(dir(ws).join(format!("{id}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())
}
pub fn save(spec: &Value, id: &str, ws: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir(ws)).map_err(|e| e.to_string())?;
    std::fs::write(dir(ws).join(format!("{id}.json")), serde_json::to_string_pretty(spec).unwrap()).map_err(|e| e.to_string())
}

pub fn validate(spec: &Value) -> Option<String> {
    let ds = spec["datasets"].as_object().filter(|d| !d.is_empty());
    let Some(ds) = ds else { return Some("spec.datasets is required (at least one {sql})".into()) };
    let ws = arr(spec, "widgets");
    if ws.is_empty() { return Some("spec.widgets is required".into()); }
    let mut ids = BTreeSet::new();
    for w in ws {
        let Some(id) = w["id"].as_str() else { return Some("every widget needs an id".into()) };
        if !ids.insert(id.to_string()) { return Some(format!("duplicate widget id {id}")); }
        let t = w["type"].as_str().unwrap_or("");
        if t != "text" && !CHART_TYPES.contains(&t) { return Some(format!("widget {id}: unknown type '{t}'")); }
        if t == "text" { continue; }
        match w["dataset"].as_str() { Some(d) if ds.contains_key(d) => {} _ => return Some(format!("widget {id}: dataset missing or unknown")) }
        for y in arr(w, "y") {
            if let Some(ag) = y["agg"].as_str() { if !AGGS.contains(&ag) { return Some(format!("widget {id}: bad agg {ag}")); } }
        }
        if let Some(e) = check_analysis(&w["options"], id) { return Some(e); }
    }
    None
}

fn check_analysis(o: &Value, id: &str) -> Option<String> {
    if o.is_null() { return None; }
    if let Some(c) = o["calc"].as_str() { if !CALCS.contains(&c) { return Some(format!("widget {id}: options.calc must be one of {CALCS:?}")); } }
    let refs: Vec<Value> = match &o["refLines"] { Value::Array(a) => a.clone(), Value::Null => vec![], v => vec![v.clone()] };
    for r in refs {
        let t = r.as_str().map(String::from).unwrap_or_else(|| sv(&r, "type"));
        if !REF_TYPES.contains(&t.as_str()) { return Some(format!("widget {id}: refLines type '{t}' must be one of {REF_TYPES:?}")); }
        if (t == "value" || t == "target") && r["value"].as_f64().is_none() { return Some(format!("widget {id}: refLines {{type:'value'}} needs a numeric value")); }
    }
    None
}

pub fn check_fields(spec: &Value, data: &Map<String, Value>) -> Option<String> {
    let mut extra: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (key, name_key) in [("calcFields", "name"), ("measures", "id")] {
        for cf in arr(spec, key) {
            let Some(n) = cf[name_key].as_str() else { continue };
            let targets: Vec<String> = cf["dataset"].as_str().map(|d| vec![d.to_string()]).unwrap_or_else(|| data.keys().cloned().collect());
            for d in targets { extra.entry(d).or_default().insert(n.into()); }
        }
    }
    let cols = |d: &str| -> BTreeSet<String> {
        let mut c: BTreeSet<String> = super::strs(&data.get(d).cloned().unwrap_or(Value::Null)["columns"]).into_iter().collect();
        c.extend(extra.get(d).cloned().unwrap_or_default());
        c
    };
    let mut problems = vec![];
    for w in arr(spec, "widgets").iter().filter(|w| w["type"] != "text") {
        let Some(d) = w["dataset"].as_str() else { continue };
        let c = cols(d);
        let mut fields: Vec<String> = ["x", "series"].iter().filter_map(|k| w[*k].as_str().map(String::from)).collect();
        fields.extend(arr(w, "y").iter().filter_map(|y| y["field"].as_str().map(String::from)));
        if let Some(f) = w["measure"]["field"].as_str() { fields.push(f.into()); }
        for k in ["dimensionOptions", "measureOptions", "breakdownOptions"] { fields.extend(super::strs(&w[k])); }
        for f in fields.iter().filter(|f| !c.contains(*f)) {
            problems.push(format!("widget {}: field '{f}' not in dataset {d} columns {:?}", sv(w, "id"), c));
        }
    }
    for f in arr(spec, "filters") {
        if let (Some(d), Some(field)) = (f["dataset"].as_str(), f["field"].as_str()) {
            if !cols(d).contains(field) { problems.push(format!("filter {}: field '{field}' not in dataset {d}", sv(f, "id"))); }
        }
    }
    if problems.is_empty() { None } else { Some(problems.into_iter().take(8).collect::<Vec<_>>().join("\n")) }
}

/// `DPExpr.validate` from expr.js on dataset columns (no rows: compile + field checks). Empty when the spec has no
/// expressions or the engine cannot load (the renderer still reports errors then).
pub fn check_exprs(spec: &Value, data: &Map<String, Value>, web_dir: &Path) -> Vec<String> {
    let has = ["calcFields", "measures", "params"].iter().any(|k| !arr(spec, k).is_empty());
    if !has { return vec![]; }
    let Ok(src) = std::fs::read_to_string(web_dir.join("expr.js")) else { return vec![] };
    let d: Map<String, Value> = data.iter().map(|(k, v)| (k.clone(), json!({"columns": v["columns"], "rows": []}))).collect();
    let sub: Map<String, Value> = ["calcFields", "measures", "params"].iter().map(|k| (k.to_string(), Value::Array(arr(spec, k).to_vec()))).collect();
    let sub = Value::Object(sub);
    let code = format!("JSON.stringify(DPExpr.validate({sub}, {}))", Value::Object(d));
    let run = || -> Option<String> {
        let rt = rquickjs::Runtime::new().ok()?;
        rt.set_memory_limit(64 << 20);
        let ctx = rquickjs::Context::full(&rt).ok()?;
        ctx.with(|ctx| {
            ctx.eval::<(), _>(src.as_str()).ok()?;
            ctx.eval::<String, _>(code.as_str()).ok()
        })
    };
    run().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default()
}

fn field_and_expr_check(spec: &Value, data: &Map<String, Value>, c: &ToolCtx) -> Option<String> {
    let probs = check_exprs(spec, data, &c.shared.paths.resources.join("web"));
    if !probs.is_empty() {
        return Some(format!("表达式有误（calcFields / measures / params）：\n{}\n请修正后重试。", probs.into_iter().take(8).collect::<Vec<_>>().join("\n")));
    }
    check_fields(spec, data)
}

pub fn auto_layout(widgets: &[Value]) -> Value {
    let (mut out, mut x, mut y, mut row_h) = (vec![], 0, 0, 0);
    for w in widgets {
        let t = w["type"].as_str().unwrap_or("");
        let (ww, hh) = if t == "kpi" { (3, 2) } else if t == "table" || t == "text" { (12, 4) } else { (6, 5) };
        if x + ww > 12 { x = 0; y += row_h; row_h = 0; }
        out.push(json!({"i": w["id"], "x": x, "y": y, "w": ww, "h": hh}));
        x += ww; row_h = row_h.max(hh);
    }
    Value::Array(out)
}

/// Run every dataset SQL (in parallel) to get columns + row counts. Rows go to a temp file in the kernel and are
/// discarded here — the app's dashboard panel hydrates the data itself when it opens the spec.
fn hydrate(spec: &Value, c: &ToolCtx) -> (Map<String, Value>, Vec<String>) {
    let sets: Vec<(String, String)> = spec["datasets"].as_object().into_iter().flatten()
        .filter_map(|(k, d)| d["sql"].as_str().map(|s| (k.clone(), s.to_string()))).collect();
    let results: Vec<(String, Result<Value, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = sets.iter().map(|(k, sql)| s.spawn(move || {
            let r = c.kernel("query_data", json!({"sql": sql, "cap": 100_000, "to_file": true}), 300);
            if let Ok(v) = &r { if let Some(f) = v["rows_file"].as_str() { let _ = std::fs::remove_file(f); } }
            (k.clone(), r)
        })).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| (String::new(), Err("panic".into())))).collect()
    });
    let (mut data, mut errors) = (Map::new(), vec![]);
    for (k, r) in results {
        match r {
            Ok(v) => { data.insert(k, json!({"columns": v["columns"], "row_count": v["row_count"]})); }
            Err(e) => errors.push(format!("dataset {k}: {e}")),
        }
    }
    errors.sort();
    (data, errors)
}

/// UI `dashboard/query`: one dataset of a saved dashboard. The SQL is read from the spec file (never taken from the
/// caller). Rows stay in the kernel's temp file (`rows_file`); the UI reads and deletes it.
pub fn query_dataset_for_ui(id: &str, dataset: &str, k: &crate::kernel::KernelClient, cap: u64) -> Result<Value, String> {
    let spec = load(id, k.workspace()).ok_or_else(|| format!("dashboard not found: {id}"))?;
    let sql = spec["datasets"][dataset]["sql"].as_str().ok_or_else(|| format!("dataset not found: {dataset}"))?;
    k.call("query_data", json!({"sql": sql, "cap": cap, "to_file": true}), 300).map(|r| r.result)
}

fn dashboard_create(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let Some(mut spec) = a["spec"].as_object().cloned() else { return Ok(ToolResult::err("spec must be an object")) };
    if let Some(e) = validate(&Value::Object(spec.clone())) { return Ok(ToolResult::err(e)); }
    let ws = c.workspace();
    let id = a["id"].as_str().map(String::from).unwrap_or_else(|| format!("db_{}", text::short_id(8)));
    spec.insert("id".into(), json!(id));
    spec.insert("version".into(), json!(2));
    match load(&id, ws) {
        Some(old) => { if !old["owner"].is_null() { spec.insert("owner".into(), old["owner"].clone()); } }
        None => { if c.shared.project_root.is_some() { spec.insert("owner".into(), json!(c.shared.id)); } }
    }
    if spec.get("layout").map(|l| l.is_null()).unwrap_or(true) {
        spec.insert("layout".into(), auto_layout(arr(&Value::Object(spec.clone()), "widgets")));
    }
    let spec = Value::Object(spec);
    let (data, errors) = hydrate(&spec, c);
    if !errors.is_empty() { return Ok(ToolResult::err(format!("数据集 SQL 执行失败：\n{}\n请修正 SQL 后重试。", errors.join("\n")))); }
    if let Some(e) = field_and_expr_check(&spec, &data, c) { return Ok(ToolResult::err(e)); }
    save(&spec, &id, ws)?;
    let rows: Vec<String> = data.iter().map(|(k, v)| format!("{k}: {} 行", v["row_count"].as_i64().unwrap_or(0))).collect();
    Ok(ToolResult::with(format!("看板已创建 id={id}，{} 个组件（{}）。用户可在看板面板中筛选、切换维度/指标/图表类型，并导出 HTML。修改时调用 dashboard_update 或用相同 id 再次 dashboard_create。",
        arr(&spec, "widgets").len(), rows.join(", ")), json!({"kind": "dashboard", "id": id, "title": spec["title"]})))
}

fn dashboard_update(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let ws = c.workspace();
    let id = sv(a, "id");
    let Some(Value::Object(mut spec)) = load(&id, ws) else { return Ok(ToolResult::err("dashboard not found")) };
    let patch = a["patch"].as_object().cloned().unwrap_or_default();
    for k in ["title", "datasets", "filters", "layout", "calcFields", "measures", "params", "palette"] {
        if let Some(v) = patch.get(k) { spec.insert(k.into(), v.clone()); }
    }
    let mut widgets: Vec<Value> = spec.get("widgets").and_then(|w| w.as_array()).cloned().unwrap_or_default();
    for w in patch.get("widgets").and_then(|w| w.as_array()).into_iter().flatten() {
        match widgets.iter_mut().find(|x| x["id"] == w["id"]) {
            Some(cur) => { if let (Some(o), Some(n)) = (cur.as_object_mut(), w.as_object()) { for (k, v) in n { o.insert(k.clone(), v.clone()); } } }
            None => widgets.push(w.clone()),
        }
    }
    let rm: BTreeSet<String> = super::strs(&a["remove"]).into_iter().collect();
    widgets.retain(|w| !rm.contains(w["id"].as_str().unwrap_or("")));
    if !patch.contains_key("layout") { spec.insert("layout".into(), auto_layout(&widgets)); }
    let n = widgets.len();
    spec.insert("widgets".into(), Value::Array(widgets));
    let spec = Value::Object(spec);
    if let Some(e) = validate(&spec) { return Ok(ToolResult::err(e)); }
    let (data, errors) = hydrate(&spec, c);
    if !errors.is_empty() { return Ok(ToolResult::err(errors.join("\n"))); }
    if let Some(e) = field_and_expr_check(&spec, &data, c) { return Ok(ToolResult::err(e)); }
    save(&spec, &id, ws)?;
    Ok(ToolResult::with(format!("看板 {id} 已更新（{n} 个组件）"), json!({"kind": "dashboard", "id": id, "title": spec["title"]})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_and_fields() {
        let spec = json!({"datasets": {"d1": {"sql": "select 1"}}, "widgets": [
            {"id": "w1", "type": "bar", "dataset": "d1", "x": "大区", "y": [{"field": "毛利率", "agg": "sum"}]},
            {"id": "w2", "type": "kpi", "dataset": "d1", "measure": {"field": "销售额"}}],
            "measures": [{"id": "毛利率", "expr": "sum(毛利)/sum(销售额)"}]});
        assert!(validate(&spec).is_none());
        let mut data = Map::new();
        data.insert("d1".into(), json!({"columns": ["大区", "销售额"]}));
        assert!(check_fields(&spec, &data).is_none());
        data.insert("d1".into(), json!({"columns": ["大区"]}));
        assert!(check_fields(&spec, &data).unwrap().contains("销售额"));
        assert!(validate(&json!({"datasets": {"d1": {}}, "widgets": [{"id": "w", "type": "pie3d", "dataset": "d1"}]})).unwrap().contains("unknown type"));
        assert_eq!(auto_layout(arr(&spec, "widgets"))[1]["x"], 6);
    }
    #[test]
    fn expr_precheck() {
        let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("Resources/web");
        let mut data = Map::new();
        data.insert("d1".into(), json!({"columns": ["大区", "销售额", "成本"]}));
        let ok = json!({"calcFields": [{"name": "毛利", "expr": "销售额 - 成本"}], "measures": [{"id": "毛利率", "expr": "sum(毛利)/sum(销售额)"}]});
        assert!(check_exprs(&ok, &data, &web).is_empty(), "{:?}", check_exprs(&ok, &data, &web));
        let bad = json!({"calcFields": [{"name": "x", "expr": "销售额 - - * 2", "dataset": "d1"}, {"name": "y", "expr": "利润 * 2", "dataset": "d1"}],
                         "measures": [{"id": "m", "expr": "销售额 / 2"}]});
        let p = check_exprs(&bad, &data, &web);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p.iter().any(|e| e.contains("利润")) && p.iter().any(|e| e.contains("must aggregate")));
        assert!(check_exprs(&json!({}), &data, &web).is_empty());
    }
}
