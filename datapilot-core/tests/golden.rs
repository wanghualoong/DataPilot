//! Golden-sample replay (tests/golden/cases/*.json) against scripts/golden_llm_server.py — the same cases and
//! normalisation as the Swift harness (Packages/AgentCore/Tests/AgentCoreTests/GoldenTests.swift).
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use datapilot_core::agent::Core;
use datapilot_core::events::Emit;
use datapilot_core::paths::Paths;
use datapilot_core::permissions::Mode;
use datapilot_core::secrets::StaticSecrets;
use datapilot_core::types::{Config, ModelRef, Provider};

fn repo() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf() }

fn required_by_type() -> HashMap<String, Vec<String>> {
    let s: Value = serde_json::from_str(&std::fs::read_to_string(repo().join("protocol/agent-event.schema.json")).unwrap()).unwrap();
    s["oneOf"].as_array().unwrap().iter().filter_map(|e| {
        let t = e["properties"]["type"]["const"].as_str()?.to_string();
        Some((t, e["required"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()))
    }).collect()
}

fn normalise(events: &[Value]) -> Value {
    let (mut rounds, mut order, mut finished, mut reason, mut text, mut compactions) = (vec![], HashMap::new(), vec![], String::new(), String::new(), 0);
    for e in events {
        match e["type"].as_str().unwrap_or("") {
            "assistant_message" => {
                let calls = e["message"]["tool_calls"].as_array().cloned().unwrap_or_default();
                rounds.push(Value::Array(calls.iter().map(|c| c["name"].clone()).collect()));
                for c in &calls { let n = order.len(); order.insert(c["id"].as_str().unwrap_or("").to_string(), n); }
            }
            "tool_finished" => {
                let mut r = json!({"tool": e["message"]["name"], "error": e["is_error"]});
                if e["display"]["kind"] == "blocked" { r["blocked"] = json!(true); }
                finished.push((*order.get(e["call_id"].as_str().unwrap_or("")).unwrap_or(&usize::MAX), r));
            }
            "assistant_delta" => text += e["text"].as_str().unwrap_or(""),
            "compacted" => compactions += 1,
            "turn_finished" => reason = e["reason"].as_str().unwrap_or("").to_string(),
            _ => {}
        }
    }
    finished.sort_by_key(|f| f.0); // stable: ties keep arrival order
    json!({"tool_calls": rounds, "results": finished.into_iter().map(|f| f.1).collect::<Vec<_>>(), "reason": reason, "text": text, "compactions": compactions})
}

fn wait_port(port: u16) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() { return; }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("golden server did not start on {port}");
}

fn run_case(path: &Path, idx: u16, required: &HashMap<String, Vec<String>>) -> Vec<String> {
    let c: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let name = path.file_stem().unwrap().to_string_lossy().to_string();
    let python = repo().join("build/vendor/python/bin/python3");
    let port = 21000 + (std::process::id() % 500) as u16 * 10 + idx;
    let mut server = Command::new(&python).arg("-I").arg(repo().join("scripts/golden_llm_server.py")).arg(port.to_string()).arg(path)
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    wait_port(port);

    let support = std::env::temp_dir().join(format!("dp-golden-rs-{}-{name}", datapilot_core::text::short_id(6)));
    let mut cfg = Config::default();
    cfg.providers.push(Provider { id: "golden".into(), name: "golden".into(), base_url: format!("http://127.0.0.1:{port}"),
                                  proxy_mode: Some("none".into()), ..Default::default() });
    cfg.roles.main = Some(ModelRef { provider_id: "golden".into(), model: "golden-model".into(), capabilities: None });
    cfg.settings.memory_enabled = false;
    let core = Core::new(Paths::dev(support.clone()), cfg, Arc::new(StaticSecrets::default())).unwrap();
    let mut agent = core.open(None, None).unwrap();
    if let Some(m) = c["mode"].as_str().and_then(Mode::parse) { agent.shared.perms.set_mode(m); }
    if c["fixtures"] == true {
        let st = Command::new(&python).arg("-I").arg(repo().join("tests/python/make_fixtures.py")).arg(agent.shared.workspace.join("uploads"))
            .stdout(Stdio::null()).status().unwrap();
        assert!(st.success(), "make_fixtures failed");
    }
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let ev = events.clone();
    let shared = agent.shared.clone();
    let interact = c["interact"].clone();
    let emit: Emit = Arc::new(move |e: Value| {
        // scripted user: answer cards immediately (the broker keeps answers that arrive before the wait)
        let ans = |k: &str| interact[k].as_str().map(String::from);
        match e["type"].as_str().unwrap_or("") {
            "approval_required" => if let Some(a) = ans("approval") { shared.broker.respond(e["request"]["id"].as_str().unwrap_or(""), &a, None) },
            "plan_proposed" => if let Some(a) = ans("plan") { shared.broker.respond(e["call_id"].as_str().unwrap_or(""), &a, None) },
            "question" => if let Some(a) = ans("question") { shared.broker.respond(e["call_id"].as_str().unwrap_or(""), &a, None) },
            _ => {}
        }
        ev.lock().unwrap().push(e)
    });
    agent.run_turn(c["user"].as_str().unwrap_or(""), None, &emit);
    agent.shutdown();
    core.shutdown();
    let _ = server.kill();
    let _ = server.wait();
    let events = events.lock().unwrap().clone();

    let mut fails = vec![];
    for e in &events {
        let t = e["type"].as_str().unwrap_or("");
        match required.get(t) {
            None => fails.push(format!("{name}: event type {t} missing from schema")),
            Some(req) => for k in req { if e.get(k).is_none() { fails.push(format!("{name}: {t} lacks required field {k}")); } },
        }
    }
    let got = normalise(&events);
    let out = repo().join("tests/golden/out");
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(out.join(format!("{name}.rust.json")), serde_json::to_string_pretty(&got).unwrap());
    let exp = &c["expect"];
    for k in ["tool_calls", "results", "reason"] {
        if got[k] != exp[k] { fails.push(format!("{name} {k}: got {} expected {}", got[k], exp[k])); }
    }
    if !exp["compactions"].is_null() && got["compactions"] != exp["compactions"] {
        fails.push(format!("{name} compactions: got {} expected {}", got["compactions"], exp["compactions"]));
    }
    if let Some(n) = exp["text_contains"].as_str() {
        if !got["text"].as_str().unwrap_or("").contains(n) { fails.push(format!("{name} text: {}", got["text"])); }
    }
    if fails.is_empty() { let _ = std::fs::remove_dir_all(&support); } else {
        for e in events.iter().filter(|e| e["type"] == "tool_finished" || e["type"] == "error") { eprintln!("{name}: {}", datapilot_core::text::prefix(&e.to_string(), 600)); }
    }
    fails
}

#[test]
fn golden_replay() {
    let required = required_by_type();
    assert!(required.len() >= 20);
    let mut cases: Vec<PathBuf> = std::fs::read_dir(repo().join("tests/golden/cases")).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false)).collect();
    cases.sort();
    assert!(!cases.is_empty());
    let fails: Vec<String> = cases.iter().enumerate().flat_map(|(i, p)| run_case(p, i as u16, &required)).collect();
    assert!(fails.is_empty(), "golden failures:\n{}", fails.join("\n"));
}

#[test]
fn registry_matches_snapshot() {
    let r = datapilot_core::tools::Registry::load(&repo().join("protocol/tools.snapshot.json")).unwrap();
    for n in ["data_import", "data_schema", "sql_query", "python", "data_transform", "exit_plan_mode", "db_query", "grep"] {
        assert!(r.get(n).is_some(), "{n} not registered");
    }
}
