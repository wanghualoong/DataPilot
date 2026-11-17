//! Data tools: thin wrappers over kernel ops (ports of the Swift DataTools / BITools subset).
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use super::{imp, pick, render_table, strs, sv, with_risk, Impl, ToolCtx, ToolResult};
use crate::permissions::Risk;
use crate::text;
use crate::types::{arr, i, s};

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("data_import", imp(data_import, |a| format!("导入 {}", sv(a, "path"))));
    m.insert("data_schema", imp(data_schema, |a| {
        let t = strs(&a["tables"]);
        format!("查看结构 {}", if t.is_empty() { "全部".to_string() } else { t.join(",") })
    }));
    m.insert("sql_query", imp(sql_query, |a| format!("SQL: {}", text::prefix(&sv(a, "sql").replace('\n', " "), 120))));
    m.insert("python", imp(python, |a| {
        let c = sv(a, "code");
        let first: Vec<&str> = c.split('\n').take(2).collect();
        format!("python: {}", text::prefix(&first.join("; "), 100))
    }));
    m.insert("data_transform", with_risk(imp(data_transform, |a| format!("派生表: {}", text::prefix(&sv(a, "sql").replace('\n', " "), 120))),
        |a| if sv(a, "sql").trim().to_lowercase().starts_with("drop") { Risk::External } else { Risk::Sandboxed }));
    m.insert("data_export", imp(data_export, |a| {
        let src = [sv(a, "table"), sv(a, "result_id"), text::prefix(&sv(a, "sql"), 60), sv(a, "kind")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
        format!("导出 {}: {src}", a["format"].as_str().unwrap_or("csv"))
    }));
    m.insert("relation_test", imp(relation_test, |a| format!("测试关联 {} → {}", side_name(&a["from"]), side_name(&a["to"]))));
    m.insert("relation_define", imp(relation_define, |a| match a["action"].as_str() {
        Some(act) => format!("关系 {act} {}", sv(a, "id")),
        None => format!("保存关系 {} → {}", side_name(&a["from"]), side_name(&a["to"])),
    }));
    m.insert("catalog_update", imp(catalog_update, |_| "更新语义层".into()));
    m.insert("chart_create", imp(chart_create, |a| format!("图表：{}（{}）", sv(a, "title"), sv(a, "type"))));
}

fn side_name(v: &Value) -> String { v.as_str().map(String::from).or_else(|| s(v, "table")).unwrap_or_default() }
fn sval(v: &Value) -> String { v.as_str().map(String::from).unwrap_or_else(|| v.to_string()) }

pub fn compact_profile(p: &Value) -> String {
    let cols = arr(p, "columns");
    let mut out: Vec<String> = cols.iter().take(40).map(|c| {
        let mut l = format!("  - {}: {}", s(c, "name").unwrap_or_default(), s(c, "type").unwrap_or_default());
        if let Some(n) = c["null_pct"].as_f64() { if n > 0.0 { l += &format!(" null{}%", text::fmt_num(n)); } }
        if let Some(v) = c["values"].as_array() {
            l += &format!(" 取值[{}{}]", v.iter().take(12).map(sval).collect::<Vec<_>>().join(", "), if v.len() > 12 { "…" } else { "" });
        } else if !c["min"].is_null() {
            l += &format!(" 范围[{} ~ {}]", sval(&c["min"]), sval(&c["max"]));
        } else if let Some(e) = c["examples"].as_array() {
            l += &format!(" 例[{}]", e.iter().take(3).map(sval).collect::<Vec<_>>().join(", "));
        }
        l
    }).collect();
    if cols.len() > 40 { out.push(format!("  …({} columns)", cols.len())); }
    out.join("\n")
}

fn data_import(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let r = c.kernel("import", pick(a, &["path", "table", "sheet", "header_row", "mode", "preview", "columns", "types"]), 900)?;
    if r["preview"].as_bool() == Some(true) {
        let text: Vec<String> = arr(&r, "tables").iter().map(|t| {
            let head = strs(&t["columns"]).iter().zip(strs(&t["dtypes"]).iter()).map(|(c, ty)| format!("{c}({ty})")).collect::<Vec<_>>().join(", ");
            let rows = arr(t, "sample").iter().take(5).map(|r| r.to_string()).collect::<Vec<_>>().join("\n");
            let w = s(t, "formula_warning").map(|w| format!("\n⚠️ {w}")).unwrap_or_default();
            format!("预览 {}{}：{} 行\n列：{head}\n样例：\n{rows}{w}", s(t, "table").unwrap_or_default(),
                    s(t, "sheet").map(|x| format!("（sheet {x}）")).unwrap_or_default(), i(t, "rows"))
        }).collect();
        return Ok(ToolResult::ok(text.join("\n\n") + "\n（仅预览，未写入。确认列名/类型后再不带 preview 导入，可用 columns 改名或选列、types 指定类型。）"));
    }
    let mut lines = vec![];
    let (mut own, mut family) = (vec![], vec![]);
    for t in arr(&r, "tables") {
        if let Some(fp) = s(t, "fingerprint").filter(|f| !f.is_empty()) {
            let fam = c.shared.index.register_dataset(&fp, &s(t, "table").unwrap_or_default(), &strs(&t["column_set"]), &c.shared.id);
            if !fam.is_empty() { lines.push("ℹ️ 识别为以前分析过的数据集（或同类报表），相关的已验证查询会自动作为参考提供。".to_string()); }
            own.push(fp);
            family.extend(fam);
        }
        let mut l = format!("✅ {}: {} 行 × {} 列，策略={}", s(t, "table").unwrap_or_else(|| "?".into()), i(t, "rows"), i(t, "columns"), s(t, "strategy").unwrap_or_default());
        if let Some(e) = s(t, "encoding") { l += &format!("，编码={e}"); }
        if let Some(sh) = s(t, "sheet") { l += &format!("，sheet={sh}"); }
        lines.push(l);
        if let Some(w) = s(t, "formula_warning") { lines.push(format!("⚠️ {w}（请把这一点告诉用户）")); }
        if let Some(p) = t.get("profile") { lines.push(compact_profile(p)); }
    }
    if !own.is_empty() { c.shared.add_datasets(&own, &family); }
    lines.push("下一步：多表时调用 data_schema 查看表间关系；再用 sql_query / python 分析。".into());
    Ok(ToolResult::with(lines.join("\n"), json!({"kind": "import", "tables": r.get("tables").cloned().unwrap_or(json!([]))})))
}

fn catalog(ws: &Path) -> Value {
    std::fs::read_to_string(ws.join("catalog.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

fn metrics_text(cat: &Value) -> Option<String> {
    let mut lines: Vec<String> = arr(cat, "metrics").iter().map(|m| format!("- {}: {}  {}", sv(m, "name"), sv(m, "expr"), sv(m, "description"))).collect();
    if let Some(o) = cat["synonyms"].as_object() { for (k, v) in o { lines.push(format!("- 同义词 {k} → {}", sval(v))); } }
    if lines.is_empty() { None } else { Some(lines.join("\n")) }
}

fn data_schema(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let mut p = pick(a, &["tables"]);
    let cat = catalog(c.workspace());
    if cat.is_object() {
        p["descriptions"] = cat["descriptions"].clone();
        if !cat["relations"].is_null() { p["relations"] = cat["relations"].clone(); }
    }
    let r = c.kernel("schema", p, 300)?;
    let mut text = sv(&r, "m_schema");
    if let Some(m) = metrics_text(&cat) { text += &format!("\n【Metrics / 指标口径】\n{m}"); }
    Ok(ToolResult::with(text.clone(), json!({"kind": "schema", "relations": r.get("relations").cloned().unwrap_or(json!([])), "m_schema": text})))
}

fn sql_query(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let r = c.kernel("sql", pick(a, &["sql", "limit"]), 300)?;
    let cols = strs(&r["columns"]);
    let rows = arr(&r, "rows");
    let n = i(&r, "row_count");
    let mut t = format!("row_count={n} ({} ms), result_id={}\n", i(&r, "ms"), sv(&r, "result_id")) + &render_table(&cols, rows, 30);
    if n as usize > rows.len() { t += &format!("\n…(仅显示前 {} 行)", rows.len()); }
    if let Some(h) = s(&r, "hint") { t += &format!("\n⚠️ {h}"); }
    if let (Some(q), Some(sql)) = (a["question"].as_str().filter(|q| q.chars().count() >= 4), a["sql"].as_str()) {
        let fps = c.shared.datasets.lock().unwrap().clone();
        if n > 0 && !fps.is_empty() {
            let fp = fps.last().unwrap();
            if let Some(id) = c.shared.index.save_qsql(fp, q, sql, &c.shared.id) { c.shared.pending_qsql.lock().unwrap().push(id); }
        }
    }
    Ok(ToolResult::with(t, json!({"kind": "sql", "sql": a["sql"], "question": a.get("question").cloned().unwrap_or(Value::Null),
        "columns": cols, "rows": rows, "row_count": n, "result_id": r["result_id"]})))
}

fn python(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let r = c.kernel_full("exec", json!({"code": a["code"]}), 600)?;
    let res = &r.result;
    let mut parts = vec![];
    if !r.stdout.is_empty() { parts.push(format!("stdout:\n{}", r.stdout)); }
    let mut display = json!({"kind": "python", "code": a["code"], "stdout": r.stdout});
    let v = &res["value"];
    if !v.is_null() {
        if v["type"] == "dataframe" {
            let cols = strs(&v["columns"]);
            parts.push(format!("DataFrame {}:\n{}", v["shape"], render_table(&cols, arr(v, "rows"), 30)));
            display["table"] = json!({"columns": cols, "rows": v["rows"], "shape": v["shape"]});
        } else if let Some(t) = v["text"].as_str() { parts.push(format!("Out: {t}")); }
    }
    let ch = arr(res, "changed_dataframes");
    if !ch.is_empty() {
        parts.push(format!("DataFrames updated: {}", ch.iter().filter_map(|c| c["name"].as_str().map(|n| format!("{n}{}", c["shape"]))).collect::<Vec<_>>().join(", ")));
    }
    let imgs = arr(res, "images");
    if !imgs.is_empty() {
        display["images"] = json!(imgs);
        parts.push(format!("Saved figures: {}", strs(&res["images"]).join(", ")));
    }
    Ok(ToolResult::with(if parts.is_empty() { "(ok, no output)".into() } else { parts.join("\n\n") }, display))
}

fn data_transform(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let r = c.kernel("transform", json!({"sql": a["sql"]}), 600)?;
    let target = sv(&r, "target");
    let kind = sv(&r, "kind");
    if kind == "drop" { return Ok(ToolResult::with(format!("已删除派生表 {target}"), json!({"kind": "transform", "target": target, "op": "drop"}))); }
    let n = i(&r, "rows");
    let cols: Vec<String> = arr(&r, "columns").iter().map(|c| format!("{} {}", sv(c, "name"), sv(c, "type"))).collect();
    let pv = &r["preview"];
    let pcols = strs(&pv["columns"]);
    let what = if kind == "update" || kind == "alter" { "已更新" } else { "已生成" };
    let obj = if kind == "create_view" { "视图" } else { "表" };
    let mut text = format!("✅ {what}派生{obj} {target}：{n} 行；列：{}", cols.join(", "));
    if r["replaced"].as_bool() == Some(true) { text += "（覆盖了旧版本）"; }
    if !pcols.is_empty() { text += &format!("\n{}", render_table(&pcols, arr(pv, "rows"), 10)); }
    Ok(ToolResult::with(text, json!({"kind": "transform", "target": target, "op": kind, "sql": a["sql"], "purpose": a.get("purpose").cloned().unwrap_or(Value::Null),
        "row_count": n, "columns": pcols, "rows": pv.get("rows").cloned().unwrap_or(json!([]))})))
}

fn human_size(b: f64) -> String {
    if b < 1000.0 { format!("{b:.0} bytes") } else if b < 1e6 { format!("{:.0} KB", b / 1e3) } else if b < 1e9 { format!("{:.1} MB", b / 1e6) } else { format!("{:.2} GB", b / 1e9) }
}

fn data_export(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let mut p = pick(a, &["sql", "table", "result_id", "format", "filename", "columns", "limit", "excel_compatible", "kind"]);
    if a["kind"] == "dictionary" {
        let cat = catalog(c.workspace());
        if cat.is_object() { p["descriptions"] = cat["descriptions"].clone(); if !cat["relations"].is_null() { p["relations"] = cat["relations"].clone(); } }
    }
    let r = c.kernel("export", p, 900)?;
    let path = sv(&r, "path");
    let rows = i(&r, "rows");
    let bytes = r["bytes"].as_f64().unwrap_or(0.0);
    let base = format!("{}/", c.shared.project_root.as_deref().unwrap_or(c.workspace()).display());
    Ok(ToolResult::with(format!("✅ 已导出 {rows} 行到 {}（{}）", path.replace(&base, ""), human_size(bytes)),
        json!({"kind": "file", "path": path, "rows": rows, "bytes": bytes, "format": r.get("format").cloned().unwrap_or(json!(""))})))
}

fn list(r: &Value, k: &str) -> String { arr(r, k).iter().map(sval).collect::<Vec<_>>().join(", ") }

fn relation_test(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let mut p = json!({"from": a["from"], "to": a["to"]});
    if !a["mapping"].is_null() { p["mapping"] = a["mapping"].clone(); }
    let r = c.kernel("relation_test", p, 300)?;
    let mut text = format!("匹配率 {:.1}%（去重后 {:.1}%），检查 {} 行，基数 {}\n", r["match_rate"].as_f64().unwrap_or(0.0) * 100.0,
        r["distinct_match_rate"].as_f64().unwrap_or(0.0) * 100.0, i(&r, "rows_checked"), s(&r, "cardinality").unwrap_or_else(|| "?".into()));
    text += &format!("from 样例: {}\nto 样例: {}\n未匹配样例: {}\nJOIN: {}", list(&r, "from_samples"), list(&r, "to_samples"), list(&r, "unmatched_samples"), sv(&r, "join_sql"));
    if let Some(sg) = r.get("suggestion").filter(|x| !x.is_null()) {
        text += &format!("\n💡 建议转换（两边都应用）：{} → 匹配率 {:.1}%。确认后用 relation_define 保存（steps={}）。", sv(sg, "label"),
            sg["match_rate"].as_f64().unwrap_or(0.0) * 100.0, sg.get("steps").cloned().unwrap_or(json!([])));
    }
    Ok(ToolResult::with(text, json!({"kind": "relation", "result": r})))
}

fn relation_define(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let mut p = pick(a, &["from", "to", "mapping_pairs", "mapping", "id", "action", "note", "min_match", "materialize"]);
    p["source"] = json!("model");
    let r = c.kernel("relation_define", p, 300)?;
    if r["ok"].as_bool() != Some(true) {
        let mut t = format!("未保存：{}", s(&r, "reason").unwrap_or_else(|| "relation not found".into()));
        let um = list(&r, "unmatched_samples");
        if !um.is_empty() { t += &format!("\n未匹配样例: {um}"); }
        if let Some(sg) = r.get("suggestion").filter(|x| !x.is_null()) { t += &format!("\n💡 建议转换：{}（steps={}）", sv(sg, "label"), sg.get("steps").cloned().unwrap_or(json!([]))); }
        t += "\n如果规则不明确，用 ask_user 把样例给用户，请用户说明对应方式或提供对照表。";
        return Ok(ToolResult::with(t, json!({"kind": "relation", "result": r})));
    }
    let mut t = "✅ 关系已保存".to_string();
    if let Some(rel) = r.get("relation").filter(|x| !x.is_null()) {
        t += &format!("：{}.{} → {}.{}（{}）", sv(rel, "from_table"), sv(rel, "from_column"), sv(rel, "to_table"), sv(rel, "to_column"), sv(rel, "status"));
        let tr = sv(rel, "transform");
        if !tr.is_empty() { t += &format!(" 转换：{tr}"); }
    }
    if let Some(m) = r["match_rate"].as_f64() { t += &format!("，匹配率 {:.1}%", m * 100.0); }
    if let Some(j) = s(&r, "join_sql") { t += &format!("\nJOIN: {j}"); }
    if let Some(v) = s(&r, "view") { t += &format!("\n已建视图 {v}（含规范化键）"); }
    Ok(ToolResult::with(t, json!({"kind": "relation", "result": r})))
}

fn catalog_update(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let mut cat = catalog(c.workspace());
    if !cat.is_object() { cat = json!({"descriptions": {}, "metrics": [], "synonyms": {}}); }
    let mut notes = vec![];
    for r in arr(a, "relations") {
        let (ft, fc, tt, tc) = (sv(r, "from_table"), sv(r, "from_column"), sv(r, "to_table"), sv(r, "to_column"));
        if ft.is_empty() || fc.is_empty() || tt.is_empty() || tc.is_empty() { continue; }
        let res = c.kernel("relation_define", json!({"from": format!("{ft}.{fc}"), "to": format!("{tt}.{tc}"), "force": true, "source": "model"}), 120).ok();
        let rate = res.and_then(|x| x["match_rate"].as_f64()).map(|m| format!(" (match {:.0}%)", m * 100.0)).unwrap_or_default();
        notes.push(format!("{ft}.{fc}→{tt}.{tc}{rate}"));
    }
    if let Some(d) = a["descriptions"].as_object() {
        if !cat["descriptions"].is_object() { cat["descriptions"] = json!({}); }
        for (t, cols) in d {
            if !cat["descriptions"][t].is_object() { cat["descriptions"][t] = json!({}); }
            for (k, v) in cols.as_object().into_iter().flatten() { cat["descriptions"][t][k] = v.clone(); }
        }
    }
    if let Some(ms) = a["metrics"].as_array() {
        let mut cur: Vec<Value> = arr(&cat, "metrics").to_vec();
        for m in ms { cur.retain(|x| x["name"] != m["name"]); cur.push(m.clone()); }
        cat["metrics"] = json!(cur);
    }
    if let Some(sy) = a["synonyms"].as_object() {
        if !cat["synonyms"].is_object() { cat["synonyms"] = json!({}); }
        for (k, v) in sy { cat["synonyms"][k] = v.clone(); }
    }
    std::fs::write(c.workspace().join("catalog.json"), serde_json::to_string_pretty(&cat).unwrap()).map_err(|e| e.to_string())?;
    let summary = format!("{} tables described, {} metrics", cat["descriptions"].as_object().map(|o| o.len()).unwrap_or(0), arr(&cat, "metrics").len());
    Ok(ToolResult::ok(format!("catalog updated: {summary}{}", if notes.is_empty() { String::new() } else { format!("; relations saved: {}", notes.join(", ")) })))
}

/// Renders the chart with the bundled ECharts page (Resources/web/index.html) in headless Chrome, writes a 2× PNG.
fn save_png(chart: &Value, name: &str, c: &ToolCtx) -> String {
    use base64::Engine as _;
    let Some((exe, proxy)) = super::web::chrome(c) else { return "PNG 未保存：未找到本机 Chrome".into() };
    let dest = match super::agent::write_url(c, name) { Ok(d) => d, Err(e) => return format!("PNG 保存失败：{e}") };
    let page = crate::chrome::file_url(&c.shared.paths.resources.join("web/index.html"));
    let script = format!("(async () => {{ window.DP_NOANIM = true; document.body.style.width = '960px'; DP.setLang('zh'); DP.setTheme('light'); DP.renderChart({chart}); await new Promise(r => setTimeout(r, 400)); return DP.exportPNG(); }})()");
    let r = crate::chrome::evaluate(&page, &exe, &c.shared.paths.app_support, &proxy, "typeof DP !== 'undefined' && typeof echarts !== 'undefined'", &script, Duration::from_secs(20));
    let data = match r {
        Ok(v) => v.as_str().and_then(|u| u.split_once(',')).and_then(|(_, b)| base64::engine::general_purpose::STANDARD.decode(b).ok()).filter(|d| !d.is_empty()),
        Err(e) => return format!("PNG 保存失败：{e}"),
    };
    let Some(png) = data else { return "PNG 保存失败：图表渲染没有返回图片".into() };
    if let Some(p) = dest.parent() { let _ = std::fs::create_dir_all(p); }
    match std::fs::write(&dest, &png) { Ok(_) => format!("PNG 已保存：{name}（{} KB）", png.len() / 1024), Err(e) => format!("PNG 保存失败：{e}") }
}

fn chart_create(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let r = c.kernel("query_data", json!({"sql": a["sql"], "cap": 5000}), 300)?;
    let cols = strs(&r["columns"]);
    for k in ["x", "series"] {
        if let Some(col) = a[k].as_str() { if !cols.iter().any(|x| x == col) { return Ok(ToolResult::err(format!("列 '{col}' 不在结果中。可用列：{cols:?}"))); } }
    }
    for col in strs(&a["y"]) { if !cols.contains(&col) { return Ok(ToolResult::err(format!("列 '{col}' 不在结果中。可用列：{cols:?}"))); } }
    let n = i(&r, "row_count");
    if n == 0 { return Ok(ToolResult::err("SQL 结果为空，无法画图。请先核对过滤条件。")); }
    let chart = json!({"kind": "chart", "spec": {"type": a["type"].as_str().unwrap_or("bar"), "title": a["title"], "x": a["x"], "y": a["y"], "series": a["series"],
        "options": if a["options"].is_object() { a["options"].clone() } else { json!({}) }}, "sql": a["sql"], "data": {"columns": r["columns"], "rows": r["rows"]}});
    let mut text = format!("图表已渲染（{n} 行数据，列 {cols:?}）。用户可在界面中切换图表类型。");
    let mut name = sv(a, "save_png").trim().to_string();
    if !name.is_empty() {
        if !name.to_lowercase().ends_with(".png") { name += ".png"; }
        if c.shared.project_root.is_none() && !name.contains('/') { name = format!("exports/{name}"); }
        text += &format!("\n{}", save_png(&chart, &name, c));
    }
    Ok(ToolResult::with(text, chart))
}
