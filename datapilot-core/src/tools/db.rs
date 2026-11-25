//! Database tools (ports of Swift DBTools): kernel ops + keychain password injected per call (never shown to the model).
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

use super::{imp, pick, render_table, strs, sv, with_risk, Impl, ToolCtx, ToolResult};
use crate::permissions::Risk;
use crate::text;
use crate::types::{arr, i, s};

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("db_list", imp(db_list, |_| "列出数据库连接".into()));
    m.insert("db_schema", imp(db_schema, |a| format!("读取数据库结构 {}", sv(a, "connection"))));
    m.insert("db_query", imp(db_query, |a| format!("查询数据库 {}：{}", sv(a, "connection"), text::prefix(&sv(a, "sql").replace('\n', " "), 100))));
    m.insert("db_extract", imp(db_extract, |a| {
        let t: Vec<String> = arr(a, "tables").iter().map(|x| x.as_str().map(String::from).or_else(|| s(x, "table")).unwrap_or_default()).collect();
        format!("从 {} 抽取 {}", sv(a, "connection"), if t.is_empty() { s(a, "as").unwrap_or_else(|| "查询结果".into()) } else { t.join(", ") })
    }));
    m.insert("db_refresh", imp(db_refresh, |a| format!("刷新 {} 的抽取表", sv(a, "connection"))));
    m.insert("query_save", with_risk(imp(query_save, |a| format!("{} 查询 {}", a["action"].as_str().unwrap_or("save"),
        s(a, "name").or_else(|| s(a, "query_id")).unwrap_or_default())),
        |a| if a["action"] == "list" { Risk::ReadOnly } else { Risk::Sandboxed }));
}

pub fn connections(ws: &Path) -> Vec<Value> {
    std::fs::read_to_string(ws.join("connections.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["connections"].as_array().cloned()).unwrap_or_default()
}

/// Connection form definitions: Resources/kernel/dataagent/db_types.json (the kernel reads the same file).
pub fn db_types(resources: &Path) -> Result<Value, String> {
    let p = resources.join("kernel/dataagent/db_types.json");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))
}

/// Saved connections as the kernel's `db_connections` returns them (plus `dialect`, `port_effective`; `ports`).
pub fn connections_for_ui(ws: &Path, types: &Value) -> Value {
    let mut ports: Vec<u64> = vec![];
    let out: Vec<Value> = connections(ws).into_iter().filter(|c| c.is_object()).map(|mut c| {
        let t = types.as_array().and_then(|a| a.iter().find(|x| x["type"] == c["type"]));
        let file = matches!(c["type"].as_str(), Some("sqlite" | "duckdb"));
        let port = if file || t.is_none() { None } else {
            c["port"].as_u64().or_else(|| c["port"].as_str().and_then(|x| x.trim().parse().ok())).filter(|p| *p > 0)
                .or_else(|| t.and_then(|t| t["default_port"].as_u64()))
        };
        ports.extend(port);
        c["dialect"] = t.map(|t| t["dialect"].clone()).unwrap_or(Value::Null);
        c["port_effective"] = json!(port);
        c
    }).collect();
    ports.sort();
    ports.dedup();
    json!({"connections": out, "ports": ports})
}

fn params(c: &ToolCtx, a: &Value) -> Result<Map<String, Value>, String> {
    let r = sv(a, "connection");
    let all = connections(c.workspace());
    let hit = all.iter().find(|x| x["id"] == r.as_str()).or_else(|| all.iter().find(|x| x["name"] == r.as_str()));
    let Some(conn) = hit else {
        let names: Vec<String> = all.iter().filter_map(|x| s(x, "name").or_else(|| s(x, "id"))).collect();
        return Err(format!("数据库连接 {r} 不存在。现有连接：{}", if names.is_empty() { "（无，请让用户在数据面板「数据库连接」中新建）".into() } else { names.join(", ") }));
    };
    let id = sv(conn, "id");
    let mut p = Map::new();
    p.insert("connection".into(), json!(id));
    if let Some(pw) = c.shared.secrets.db_password(&id).filter(|p| !p.is_empty()) { p.insert("password".into(), json!(pw)); }
    c.shared.kernel.process.sync_db_ports();
    crate::ssh_tunnel::inject(conn, c.workspace(), &mut p, c.shared.secrets.ssh_secret(&id), &c.shared.paths.app_support)?;
    Ok(p)
}

fn merge(mut p: Map<String, Value>, extra: Value) -> Value {
    if let Value::Object(e) = extra { for (k, v) in e { p.insert(k, v); } }
    Value::Object(p)
}

fn db_list(_a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let conns = connections(c.workspace());
    if conns.is_empty() {
        return Ok(ToolResult::ok("还没有数据库连接。请用户在右侧数据面板「数据库连接」中点「新建」添加（需要主机、端口、库名、账号；建议只读账号）。"));
    }
    let lines: Vec<String> = conns.iter().map(|x| {
        let loc = s(x, "path").unwrap_or_else(|| format!("{}:{}/{}", sv(x, "host"), x["port"].as_i64().map(|p| p.to_string()).unwrap_or_default(), sv(x, "database")));
        let ex: Vec<String> = arr(x, "extracts").iter().filter_map(|e| s(e, "as").map(|n| format!("{n}({} 行)", i(e, "rows")))).collect();
        format!("- {} [id={}] {} {loc}；实时查询：{}{}", sv(x, "name"), sv(x, "id"), sv(x, "type"),
                if x["live"] == false { "关" } else { "开" }, if ex.is_empty() { String::new() } else { format!("；已抽取：{}", ex.join(", ")) })
    }).collect();
    Ok(ToolResult::ok(lines.join("\n") + "\n用 db_schema 看表结构；分析时优先 db_extract 抽取到本地再用 sql_query；简单查数可用 db_query。"))
}

fn db_schema(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let p = merge(params(c, a)?, pick(a, &["tables"]));
    let r = c.kernel("db_schema", p, 300)?;
    Ok(ToolResult::with(sv(&r, "text"), json!({"kind": "db_schema", "connection": r["connection"], "foreign_keys": r.get("foreign_keys").cloned().unwrap_or(json!([]))})))
}

fn db_query(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let p = merge(params(c, a)?, pick(a, &["sql", "dialect", "limit"]));
    let r = c.kernel("db_query", p, 400)?;
    let cols = strs(&r["columns"]);
    let rows = arr(&r, "rows");
    let n = i(&r, "row_count");
    let mut t = format!("row_count={n} ({} ms), result_id={}, dialect={}\n", i(&r, "ms"), sv(&r, "result_id"), sv(&r, "dialect"));
    if r["sql"] != a["sql"] { t += &format!("实际执行（已转写）：{}\n", sv(&r, "sql")); }
    t += &render_table(&cols, rows, 30);
    if r["capped"] == true { t += &format!("\n⚠️ 达到行数上限 {}，结果不完整；需要全部数据请用 db_extract。", i(&r, "limit")); }
    else if n as usize > rows.len() { t += &format!("\n…(仅显示前 {} 行)", rows.len()); }
    Ok(ToolResult::with(t, json!({"kind": "sql", "sql": r.get("sql").cloned().unwrap_or(a["sql"].clone()), "question": a.get("question").cloned().unwrap_or(Value::Null),
        "columns": cols, "rows": rows, "row_count": n, "result_id": r["result_id"], "connection": r["connection"], "dialect": r["dialect"]})))
}

pub fn describe(r: &Value) -> String {
    let mut lines: Vec<String> = arr(r, "tables").iter().map(|t| format!("✅ {} ← {}：{} 行", sv(t, "as"), s(t, "table").unwrap_or_else(|| "查询".into()), i(t, "rows"))).collect();
    for rel in arr(r, "relations") {
        lines.push(format!("🔗 外键 → 已确认关系：{}.{} → {}.{}", sv(rel, "from_table"), sv(rel, "from_column"), sv(rel, "to_table"), sv(rel, "to_column")));
    }
    let rb = strs(&r["derived"]["rebuilt"]);
    if !rb.is_empty() { lines.push(format!("♻️ 已重建派生表：{}", rb.join(", "))); }
    for f in arr(&r["derived"], "failed") { lines.push(format!("⚠️ 派生表 {} 重建失败：{}", sv(f, "name"), sv(f, "error"))); }
    for d in arr(&r["revalidate"], "dropped") {
        let pct = |v: &Value| v.as_f64().map(|x| format!("{:.0}%", x * 100.0)).unwrap_or_else(|| "?".into());
        lines.push(format!("⚠️ 关系匹配率下降：{}（{} → {}）", sv(d, "relation"), pct(&d["before"]), pct(&d["after"])));
    }
    if let Some(n) = s(r, "note") { lines.push(n); }
    lines.join("\n") + "\n用 data_schema / sql_query 在本地分析这些表。"
}

fn db_extract(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let p = merge(params(c, a)?, pick(a, &["tables", "sql", "as", "limit"]));
    let t0 = std::time::Instant::now();
    let r = c.kernel("db_extract", p, 3600);
    let ms = t0.elapsed().as_millis().to_string();
    match &r {
        Ok(v) => {
            let tables = v["tables"].as_array().map(|a| a.len()).unwrap_or(0);
            let rows: u64 = v["tables"].as_array().into_iter().flatten().filter_map(|t| t["rows"].as_u64()).sum();
            crate::log::info("db_extract", &c.shared.id, &[("tables", tables.to_string()), ("rows", rows.to_string()), ("ms", ms)]);
        }
        Err(e) => crate::log::warn("db_extract", &c.shared.id, &[("ok", "0".into()), ("ms", ms), ("err", crate::log::err_text(e))]),
    }
    let r = r?;
    Ok(ToolResult::with(describe(&r), json!({"kind": "db_extract", "result": r})))
}

fn db_refresh(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let p = merge(params(c, a)?, pick(a, &["tables"]));
    let r = c.kernel("db_refresh", p, 3600)?;
    Ok(ToolResult::with(describe(&r), json!({"kind": "db_extract", "result": r})))
}

fn query_save(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let action = a["action"].as_str().unwrap_or("save").to_string();
    let mut p = pick(a, &["name", "sql", "connection", "note", "query_id"]);
    p["action"] = json!(action);
    let r = c.kernel("queries", p, 30)?;
    Ok(ToolResult::ok(match action.as_str() {
        "list" => {
            let qs = arr(&r, "queries");
            if qs.is_empty() { "还没有保存的查询。".to_string() } else {
                qs.iter().map(|q| format!("- {} [id={}]{}：{}", sv(q, "name"), sv(q, "id"), s(q, "connection").map(|c| format!("（{c}）")).unwrap_or_default(), sv(q, "sql"))).collect::<Vec<_>>().join("\n")
            }
        }
        "delete" => "已删除。".into(),
        _ => format!("✅ 已保存到「我的查询」：{}", sv(&r["query"], "name")),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn db_types_and_connections_without_kernel() {
        let res = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Resources");
        let types = db_types(&res).unwrap();
        let names: Vec<&str> = types.as_array().unwrap().iter().filter_map(|t| t["type"].as_str()).collect();
        assert_eq!(names, ["postgres", "mysql", "sqlserver", "oracle", "clickhouse", "sqlite", "duckdb"]);
        let ws = std::env::temp_dir().join(format!("dp-dbconn-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("connections.json"), r#"{"version":1,"connections":[
            {"id":"a","type":"mysql","host":"h"},{"id":"b","type":"postgres","port":6543},
            {"id":"c","type":"sqlite","path":"x.db"},{"id":"d","type":"mysql","port":"3306"}]}"#).unwrap();
        let r = connections_for_ui(&ws, &types);
        let c = r["connections"].as_array().unwrap();
        assert_eq!(c[0]["port_effective"], 3306);
        assert_eq!(c[0]["dialect"], "mysql");
        assert_eq!(c[1]["port_effective"], 6543);
        assert!(c[2]["port_effective"].is_null());
        assert_eq!(r["ports"], json!([3306, 6543]));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
