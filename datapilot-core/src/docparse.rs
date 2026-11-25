//! Document parsing (OCR) of scanned PDF pages and images through the user's MinerU server (`settings.docParse`).
//! The kernel decides which pages have no text layer (`ocr_scan`); only those pages are sent. The primary tier
//! (default advanced) and the cross-check tier (default basic) run in parallel; the kernel merges them (ocr.py).
//! Results are cached under AppSupport/ocr-cache by file hash + pages + tier + server version.
//! MinerU ≥ 4.2: task API (POST /v1/tasks → PUT upload → poll → result zip); 4.0 / 4.1: POST /v1/file_parse.
//! The server never gets the file name (sent as `document.<ext>`); there is no delete API — results stay on the
//! server for its retention period.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::kernel::KernelClient;
use crate::secrets::SecretStore;
use crate::types::Settings;

pub const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];
/// keychain api_key account of the (optional) MinerU API key
pub const KEY_ID: &str = "docparse";
const TIERS: &[&str] = &["flash", "basic", "standard", "advanced"];
const MAX_BYTES: u64 = 300 << 20;
const CACHE_KEEP: usize = 300;

pub fn is_image(ext: &str) -> bool { IMAGE_EXT.contains(&ext.to_lowercase().as_str()) }
pub fn handles(ext: &str) -> bool { ext.eq_ignore_ascii_case("pdf") || is_image(ext) }

/// What a caller needs: the kernel (page scan), settings, secrets, cache dir, Stop flag, progress sink.
pub struct Env<'a> {
    pub kernel: &'a KernelClient,
    pub settings: &'a Settings,
    pub secrets: &'a dyn SecretStore,
    pub app_support: &'a Path,
    pub cancel: &'a AtomicBool,
    pub progress: &'a (dyn Fn(&str) + Sync),
}

#[derive(Clone, Debug)]
pub struct Cfg { pub server: String, pub primary: String, pub check: Option<String>, pub api_key: Option<String> }

fn tier(t: &str, default: &str) -> String { if TIERS.contains(&t) { t.to_string() } else { default.to_string() } }

/// None when document parsing is turned off. `server` is "" when neither the settings nor the built-in defaults
/// (`defaults::get().doc_parse_server`) name a server.
pub fn cfg(s: &Settings, secrets: &dyn SecretStore) -> Option<Cfg> {
    let d = &s.doc_parse;
    if !d.enabled { return None; }
    let server = d.server.trim().trim_end_matches('/');
    let server = if server.is_empty() { crate::defaults::get().doc_parse_server.trim_end_matches('/').to_string() } else if server.contains("://") { server.to_string() } else { format!("http://{server}") };
    let primary = tier(&d.primary_tier, "advanced");
    let check = Some(tier(&d.check_tier, "basic")).filter(|c| d.cross_check && *c != primary);
    Some(Cfg { server, primary, check, api_key: secrets.api_key(KEY_ID).filter(|k| !k.trim().is_empty()) })
}

/// localhost / private-network address (allowed in local-only mode).
pub fn is_private(server: &str) -> bool {
    let Some((h, _)) = crate::model_sync::host_port(server, false) else { return false };
    if h == "localhost" || h.ends_with(".localhost") { return true; }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback() || a.is_private() || a.is_link_local(),
        Ok(std::net::IpAddr::V6(a)) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

fn agent(c: &Cfg, s: &Settings, secs: u64) -> Result<ureq::Agent, String> { crate::llm::http_agent(&c.server, s, secs).map(|x| x.0) }

fn url(c: &Cfg, u: &str) -> String { if u.starts_with("http://") || u.starts_with("https://") { u.to_string() } else { format!("{}{}", c.server, u) } }

fn req(a: &ureq::Agent, c: &Cfg, method: &str, u: &str) -> ureq::Request {
    let r = a.request(method, &url(c, u));
    match &c.api_key { Some(k) => r.set("Authorization", &format!("Bearer {k}")), None => r }
}

/// Body of an error / success response as JSON (with a readable message for errors).
fn done(res: Result<ureq::Response, ureq::Error>, what: &str) -> Result<Value, String> {
    match res {
        Ok(r) => { let t = r.into_string().map_err(|e| format!("{what}: {e}"))?; serde_json::from_str(&t).map_err(|_| format!("{what}: 返回的不是 JSON：{}", crate::text::prefix(&t, 200))) }
        Err(ureq::Error::Status(code, r)) => {
            let t = r.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
            let m = v["error"]["message"].as_str().or(v["detail"].as_str()).map(String::from).unwrap_or_else(|| crate::text::prefix(&t, 200));
            Err(format!("{what}: HTTP {code} {m}"))
        }
        Err(e) => Err(format!("{what}: {e}")),
    }
}

/// GET /v1/health (+ /v1/tiers): version, device, available tiers.
pub fn health(c: &Cfg, s: &Settings) -> Result<Value, String> {
    let a = agent(c, s, 15)?;
    let h = done(req(&a, c, "GET", "/v1/health").call(), "连接失败")?;
    let tiers: Vec<String> = done(req(&a, c, "GET", "/v1/tiers").call(), "tiers").ok()
        .and_then(|t| t["data"].as_array().map(|d| d.iter().filter_map(|x| x["id"].as_str().map(String::from)).collect())).unwrap_or_default();
    Ok(json!({"version": h["version"], "device": h["runtime"]["device"], "status": h["status"], "tiers": tiers}))
}

fn old_api(version: &str) -> bool {
    let mut it = version.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    a == 4 && b < 2
}

/// structured_content.json → {"<page_idx>": [{type, content, bbox}]} (only what the merge / original-page view use).
pub fn compact(sc: &Value) -> Value {
    let mut out = Map::new();
    for (i, pg) in sc["pages"].as_array().into_iter().flatten().enumerate() {
        let idx = pg["page_idx"].as_u64().unwrap_or(i as u64);
        let blocks: Vec<Value> = pg["blocks"].as_array().into_iter().flatten()
            .map(|b| json!({"type": b["type"], "content": b["content"].as_str().unwrap_or(""), "bbox": b["bbox"]})).collect();
        out.insert(idx.to_string(), Value::Array(blocks));
    }
    Value::Object(out)
}

fn wait(secs: f64, cancel: &AtomicBool) -> Result<(), String> {
    let t = Instant::now();
    while t.elapsed().as_secs_f64() < secs {
        if cancel.load(Ordering::SeqCst) { return Err("已停止 / cancelled".into()); }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub struct Job<'a> { pub bytes: &'a [u8], pub sha: &'a str, pub ext: &'a str, pub range: Option<&'a str> }

/// One tier → compacted pages.
fn parse_tier(c: &Cfg, s: &Settings, j: &Job, tier: &str, version: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str) + Sync)) -> Result<Value, String> {
    let name = format!("document.{}", j.ext.to_lowercase());
    if old_api(version) {
        let a = agent(c, s, 960)?;
        let mut r = req(&a, c, "POST", "/v1/file_parse").query("filename", &name).query("tier", tier).query("ocr_mode", "auto")
            .query("output_formats", "structured_content").query("wait_timeout", "900").query("response_format", "json")
            .set("Content-Type", "application/octet-stream");
        if let Some(pr) = j.range { r = r.query("page_range", pr); }
        let v = done(r.send_bytes(j.bytes), &format!("{tier} 档识别失败"))?;
        let f = &v["files"][0];
        if let Some(m) = f["error"]["message"].as_str() { return Err(format!("{tier} 档识别失败：{m}")); }
        return Ok(compact(&f["content"]["structured_content"]));
    }
    let a = agent(c, s, 300)?;
    let mut file = json!({"file_name": name, "bytes": j.bytes.len(), "sha256sum": j.sha});
    if let Some(pr) = j.range { file["page_range"] = json!(pr); }
    let body = json!({"tier": tier, "ocr_mode": "auto", "output_formats": ["default"], "files": [file]});
    let t = done(req(&a, c, "POST", "/v1/tasks").set("Content-Type", "application/json").send_string(&body.to_string()), &format!("{tier} 档创建任务失败"))?;
    let tid = t["task_id"].as_str().ok_or("任务 ID 缺失")?.to_string();
    let f = &t["files"][0];
    let up = &f["upload"];
    let up_url = up["url"].as_str().map(String::from).unwrap_or_else(|| format!("/v1/tasks/{tid}/uploads/{}", f["file_id"].as_str().unwrap_or("")));
    let mut r = req(&a, c, "PUT", &up_url);
    let mut ctype = false;
    if let Some(h) = up["headers"].as_object() {
        for (k, v) in h { if let Some(v) = v.as_str() { ctype |= k.eq_ignore_ascii_case("content-type"); r = r.set(k, v); } }
    }
    if !ctype { r = r.set("Content-Type", "application/octet-stream"); }
    done(r.send_bytes(j.bytes), &format!("{tier} 档上传失败")).or_else(|e| if e.contains("返回的不是 JSON") { Ok(Value::Null) } else { Err(e) })?;
    let t0 = Instant::now();
    let d = loop {
        if let Err(e) = wait(1.0, cancel) { let _ = req(&a, c, "POST", &format!("/v1/tasks/{tid}/cancel")).call(); return Err(e); }
        let d = done(req(&a, c, "GET", &format!("/v1/tasks/{tid}")).call(), "查询任务失败")?;
        match d["status"].as_str().unwrap_or("") {
            "completed" | "partial" | "failed" | "canceled" => break d,
            st => if t0.elapsed() > Duration::from_secs(1200) { return Err(format!("{tier} 档识别超时（{st}）")); }
        }
        let p = &d["progress"];
        progress(&format!("{tier} 档识别中… {:.0}s{}", t0.elapsed().as_secs_f64(), p["total"].as_u64().map(|n| format!("（{}/{n}）", p["completed"].as_u64().unwrap_or(0))).unwrap_or_default()));
    };
    let fd = &d["files"][0];
    let Some(ru) = fd["result_url"].as_str() else {
        let m = fd["error"]["message"].as_str().or(d["error"]["message"].as_str()).unwrap_or("没有结果");
        return Err(format!("{tier} 档识别失败（{}）：{m}", d["status"].as_str().unwrap_or("")));
    };
    let resp = req(&a, c, "GET", ru).call().map_err(|e| format!("{tier} 档下载结果失败：{e}"))?;
    let mut buf = vec![];
    resp.into_reader().take(MAX_BYTES).read_to_end(&mut buf).map_err(|e| format!("{tier} 档下载结果失败：{e}"))?;
    let sc = structured_from_zip(&buf).map_err(|e| format!("{tier} 档结果无法解析：{e}"))?;
    Ok(compact(&sc))
}

/// `structured_content.json` inside the result zip (only that entry is read; nothing is written to disk).
pub fn structured_from_zip(buf: &[u8]) -> Result<Value, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf)).map_err(|e| e.to_string())?;
    let name = (0..z.len()).filter_map(|i| z.name_for_index(i).map(String::from)).find(|n| n.rsplit('/').next() == Some("structured_content.json"))
        .ok_or("结果里没有 structured_content.json")?;
    let mut f = z.by_name(&name).map_err(|e| e.to_string())?;
    let mut t = String::new();
    f.by_ref().take(MAX_BYTES).read_to_string(&mut t).map_err(|e| e.to_string())?;
    serde_json::from_str(&t).map_err(|e| e.to_string())
}

fn cache_dir(app_support: &Path) -> PathBuf { app_support.join("ocr-cache") }

fn cache_path(app_support: &Path, sha: &str, tier: &str, range: Option<&str>, version: &str, server: &str) -> PathBuf {
    let k = format!("{sha}|{tier}|{}|{version}|{server}", range.unwrap_or("all"));
    let h = hex(&Sha256::digest(k.as_bytes()))[..32].to_string();
    cache_dir(app_support).join(format!("{h}.json"))
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

fn cache_put(p: &Path, v: &Value) {
    let Some(dir) = p.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() { return; }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() { let _ = std::fs::rename(&tmp, p); }
    // keep the newest CACHE_KEEP results
    let mut es: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if es.len() > CACHE_KEEP {
        es.sort();
        for (_, p) in es.iter().take(es.len() - CACHE_KEEP) { let _ = std::fs::remove_file(p); }
    }
}

fn unavailable(pages: &[u64], msg: String) -> Value { json!({"pages": pages, "unavailable": msg}) }

/// Scanned pages of `path` (within `pages`) and their OCR results for the kernel (`ocr` parameter of extract_text /
/// import), or None when every page has a text layer. Service problems come back as `{"pages", "unavailable"}` so
/// the caller can still say clearly why the text is missing (never an empty result).
pub fn ocr_for(env: &Env, path: &Path, pages: Option<&str>) -> Result<Option<Value>, String> {
    let t0 = Instant::now();
    let r = ocr_inner(env, path, pages);
    if let Ok(None) = r { return r; }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut f = vec![("ext", ext), ("ms", t0.elapsed().as_millis().to_string())];
    let warn = match &r {
        Ok(Some(o)) => {
            f.push(("pages", o["pages"].as_array().map(|a| a.len()).unwrap_or(0).to_string()));
            if let Some(t) = o["primary_tier"].as_str() { f.push(("tier", t.to_string())); }
            match o["unavailable"].as_str().or(o["primary_error"].as_str()).or(o["check_error"].as_str()) {
                Some(e) => { f.push(("err", crate::log::err_text(e))); true }
                None => false,
            }
        }
        Err(e) => { f.push(("err", crate::log::err_text(e))); true }
        Ok(None) => false,
    };
    crate::log::log(if warn { "WARN" } else { "INFO" }, "ocr", "", &f);
    r
}

fn ocr_inner(env: &Env, path: &Path, pages: Option<&str>) -> Result<Option<Value>, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !handles(&ext) { return Ok(None); }
    let mut sp = json!({"path": path.display().to_string()});
    if let Some(p) = pages { sp["pages"] = json!(p); }
    let scan = env.kernel.call_cancellable("ocr_scan", sp, 300, env.cancel)?.result;
    let scanned: Vec<u64> = scan["scanned"].as_array().into_iter().flatten().filter_map(|v| v.as_u64()).collect();
    if scanned.is_empty() { return Ok(None); }
    let image = scan["image"].as_bool().unwrap_or(false);
    let range = (!image).then(|| scan["range"].as_str().unwrap_or("").to_string()).filter(|r| !r.is_empty());
    let what = if image { "图片".to_string() } else { format!("第 {} 页是扫描件", range.as_deref().unwrap_or("")) };
    let Some(c) = cfg(env.settings, env.secrets) else {
        return Ok(Some(unavailable(&scanned, format!("{what}，需要文档识别（OCR），但已在 设置 → 通用 → 文档识别 中关闭"))));
    };
    if c.server.is_empty() {
        return Ok(Some(unavailable(&scanned, format!("{what}，需要文档识别（OCR），但还没有配置识别服务器：请在 设置 → 通用 → 文档识别 填写 MinerU 服务器地址"))));
    }
    if env.settings.local_only && !is_private(&c.server) {
        return Ok(Some(unavailable(&scanned, format!("{what}；当前为「仅本地模型」模式，文档识别服务 {} 不是本机 / 内网地址，未发送", c.server))));
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES { return Ok(Some(unavailable(&scanned, format!("{what}，文件超过 {} MB，未发送识别", MAX_BYTES >> 20)))); }
    let version = match health(&c, env.settings) {
        Ok(h) => h["version"].as_str().unwrap_or("").to_string(),
        Err(e) => return Ok(Some(unavailable(&scanned, format!(
            "{what}，需要文档识别服务，但连不上 {}（{e}）。请检查 VPN / 网络，或在 设置 → 通用 → 文档识别 中修改服务器地址", c.server)))),
    };
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let sha = hex(&Sha256::digest(&bytes));
    let job = Job { bytes: &bytes, sha: &sha, ext: &ext, range: range.as_deref() };
    let n = scanned.len();
    (env.progress)(&format!("{what}，文档识别中（{n} 页，{}{}）…", c.primary, c.check.as_ref().map(|t| format!(" + {t} 核对")).unwrap_or_default()));
    let run = |tier: &str| -> Result<Value, String> {
        let cp = cache_path(env.app_support, &sha, tier, job.range, &version, &c.server);
        if let Some(v) = std::fs::read_to_string(&cp).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) { return Ok(v); }
        let v = parse_tier(&c, env.settings, &job, tier, &version, env.cancel, env.progress)?;
        cache_put(&cp, &v);
        Ok(v)
    };
    let (p, k) = std::thread::scope(|sc| {
        let h = c.check.as_deref().map(|t| sc.spawn(move || run(t)));
        let p = run(&c.primary);
        (p, h.map(|h| h.join().unwrap_or_else(|_| Err("识别线程异常".into()))))
    });
    if env.cancel.load(Ordering::SeqCst) { return Err("已停止 / cancelled".into()); }
    let mut o = json!({"pages": scanned, "server": version, "primary_tier": c.primary});
    match (p, k) {
        (Ok(p), Some(Ok(k))) => { o["primary"] = p; o["check"] = k; o["check_tier"] = json!(c.check); }
        (Ok(p), None) => { o["primary"] = p; }
        (Ok(p), Some(Err(e))) => { o["primary"] = p; o["check_error"] = json!(e); }
        (Err(e), Some(Ok(k))) => { o["primary"] = k; o["primary_tier"] = json!(c.check); o["primary_error"] = json!(e); }
        (Err(e), _) => return Ok(Some(unavailable(&scanned, format!("{what}，文档识别失败：{e}")))),
    }
    Ok(Some(o))
}

/// `ocr_for` from a tool call: progress goes to the tool card, Stop cancels.
pub fn for_tool(c: &crate::tools::ToolCtx, path: &Path, pages: Option<&str>) -> Result<Option<Value>, String> {
    let s = &c.shared;
    let (emit, call) = (c.emit.clone(), c.call_id.clone());
    let prog = move |t: &str| emit(crate::events::tool_progress(&call, t));
    let env = Env { kernel: c.kernel_client(), settings: &s.settings, secrets: s.secrets.as_ref(), app_support: &s.paths.app_support, cancel: &s.broker.cancel, progress: &prog };
    ocr_for(&env, path, pages)
}

/// Card data (`display.ocr`): pages, tiers, review items per page, or why OCR was not possible.
pub fn card(path: &Path, o: &Value, review: &Value) -> Value {
    json!({"path": path.display().to_string(), "pages": o["pages"], "server": o["server"], "primary_tier": o["primary_tier"], "check_tier": o["check_tier"],
           "unavailable": o["unavailable"], "check_error": o["check_error"], "primary_error": o["primary_error"],
           "review": if review.is_object() { review.clone() } else { json!({}) }})
}

/// Short line for tool results / cards: which pages were OCR'd, by which tiers.
pub fn describe(o: &Value) -> String {
    let pages: Vec<u64> = o["pages"].as_array().into_iter().flatten().filter_map(|v| v.as_u64()).collect();
    if let Some(u) = o["unavailable"].as_str() { return format!("⚠️ {u}"); }
    let mut s = format!("第 {} 页为扫描件 / 图片，已由文档识别服务识别（MinerU {}，{}{}）", range_str(&pages), o["server"].as_str().unwrap_or(""),
                        o["primary_tier"].as_str().unwrap_or(""), o["check_tier"].as_str().map(|t| format!(" 为主、{t} 核对")).unwrap_or_default());
    if let Some(e) = o["check_error"].as_str() { s += &format!("；核对档失败（{e}），未做交叉核对"); }
    if let Some(e) = o["primary_error"].as_str() { s += &format!("；主档失败（{e}），改用核对档结果"); }
    s
}

pub fn range_str(nums: &[u64]) -> String {
    let mut n: Vec<u64> = nums.to_vec();
    n.sort();
    n.dedup();
    let mut out = vec![];
    let mut i = 0;
    while i < n.len() {
        let mut j = i;
        while j + 1 < n.len() && n[j + 1] == n[j] + 1 { j += 1; }
        out.push(if i == j { n[i].to_string() } else { format!("{}-{}", n[i], n[j]) });
        i = j + 1;
    }
    out.join(",")
}

/// Kernel's `ocr_review` ({page: [items]}) → one paragraph for the model (asks it to tell the user).
pub fn review_note(review: &Value) -> Option<String> {
    let m = review.as_object().filter(|m| !m.is_empty())?;
    let mut pages: Vec<(u64, usize)> = m.iter().map(|(k, v)| (k.parse().unwrap_or(0), v.as_array().map(|a| a.len()).unwrap_or(0))).collect();
    pages.sort();
    let total: usize = pages.iter().map(|x| x.1).sum();
    Some(format!("⚠️ 识别结果有 {total} 处需核对（第 {} 页，见各页末尾「需核对」）：两档识别不一致或文字被印章遮挡。引用这些内容时请明确提示用户对照原页核对（界面上可点「原页对照」）。",
                 pages.iter().map(|x| x.0.to_string()).collect::<Vec<_>>().join("、")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn helpers() {
        assert!(is_private("http://10.0.3.7:8010") && is_private("http://localhost:8010") && is_private("http://[fd00::1]:80") && is_private("192.168.1.2"));
        assert!(!is_private("https://mineru.example.com") && !is_private("http://8.8.8.8"));
        assert!(old_api("4.0.12") && old_api("4.1.0") && !old_api("4.2.3") && !old_api("5.0") && !old_api(""));
        assert_eq!(range_str(&[1, 2, 3, 5, 7, 8]), "1-3,5,7-8");
        let sc = json!({"pages": [{"page_idx": 2, "blocks": [{"type": "text", "content": "甲方", "bbox": [0, 0, 1, 1], "image_source": "x"}]}]});
        assert_eq!(compact(&sc), json!({"2": [{"type": "text", "content": "甲方", "bbox": [0, 0, 1, 1]}]}));
        let mut s = Settings::default();
        let sec = crate::secrets::StaticSecrets::default();
        let c = cfg(&s, &sec).unwrap();
        // no server in the settings and no built-in default (neutral defaults.json): empty, the caller asks for one
        assert_eq!((c.server.as_str(), c.primary.as_str(), c.check.as_deref()), ("", "advanced", Some("basic")));
        s.doc_parse.server = "mineru.lan:8010/".into();
        s.doc_parse.cross_check = false;
        let c = cfg(&s, &sec).unwrap();
        assert_eq!((c.server.as_str(), c.check.as_deref()), ("http://mineru.lan:8010", None));
        s.doc_parse.enabled = false;
        assert!(cfg(&s, &sec).is_none());
        assert!(review_note(&json!({})).is_none());
        assert!(review_note(&json!({"3": ["a", "b"], "1": ["c"]})).unwrap().contains("3 处需核对（第 1、3 页"));
    }

    /// Minimal MinerU 4.2 task API on a local port: create → upload → poll → zip result; both tiers, then the cache.
    #[test]
    fn task_api_round_trip_and_cache() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let zip_for = |tier: &str| {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(vec![]));
            w.start_file("x/structured_content.json", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(json!({"pages": [{"page_idx": 0, "blocks": [{"type": "text", "content": format!("{tier} 文字")}]}]}).to_string().as_bytes()).unwrap();
            w.finish().unwrap().into_inner()
        };
        let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let h2 = hits.clone();
        let b2 = base.clone();
        std::thread::spawn(move || {
            let mut polls = std::collections::HashMap::<String, u32>::new();
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut buf = vec![0u8; 1 << 16];
                let mut data = vec![];
                // read headers + body (Content-Length)
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 { break; }
                    data.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&data).to_string();
                    if let Some(i) = t.find("\r\n\r\n") {
                        let cl = t[..i].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if data.len() >= i + 4 + cl { break; }
                    }
                }
                let t = String::from_utf8_lossy(&data).to_string();
                let line = t.lines().next().unwrap_or("").to_string();
                let body_start = t.find("\r\n\r\n").map(|i| i + 4).unwrap_or(t.len());
                h2.lock().unwrap().push(line.clone());
                let (ctype, body): (&str, Vec<u8>) = if line.starts_with("GET /v1/health") {
                    ("application/json", json!({"status": "ok", "version": "4.2.3", "runtime": {"device": "cuda"}}).to_string().into_bytes())
                } else if line.starts_with("GET /v1/tiers") {
                    ("application/json", json!({"data": [{"id": "basic"}, {"id": "advanced"}]}).to_string().into_bytes())
                } else if line.starts_with("POST /v1/tasks ") {
                    let req: Value = serde_json::from_str(&t[body_start..]).unwrap_or(Value::Null);
                    let tier = req["tier"].as_str().unwrap_or("?").to_string();
                    assert_eq!(req["files"][0]["file_name"], "document.pdf");
                    assert_eq!(req["files"][0]["page_range"], "1");
                    ("application/json", json!({"task_id": format!("job_{tier}"), "files": [{"file_id": format!("f_{tier}"), "upload": {"url": format!("{b2}/v1/tasks/job_{tier}/uploads/f_{tier}"), "headers": {"X-Up": "1"}}}]}).to_string().into_bytes())
                } else if line.starts_with("PUT ") {
                    assert!(t.contains("X-Up: 1") || t.contains("x-up: 1"));
                    ("application/json", b"{}".to_vec())
                } else if line.starts_with("GET /v1/tasks/job_") {
                    let tier = line.split("job_").nth(1).unwrap().split(' ').next().unwrap().to_string();
                    let n = polls.entry(tier.clone()).or_insert(0);
                    *n += 1;
                    let st = if *n < 2 { "running" } else { "completed" };
                    ("application/json", json!({"status": st, "files": [{"result_url": format!("/v1/files/f_{tier}/content")}]}).to_string().into_bytes())
                } else if line.starts_with("GET /v1/files/f_") {
                    let tier = line.split("f_").nth(1).unwrap().split('/').next().unwrap().to_string();
                    ("application/zip", zip_for(&tier))
                } else { ("text/plain", b"nf".to_vec()) };
                let code = if ctype == "text/plain" { "404 Not Found" } else { "200 OK" };
                let _ = s.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                let _ = s.write_all(&body);
            }
        });
        let mut s = Settings::default();
        s.proxy_mode = "none".into();
        s.doc_parse.server = base.clone();
        let sec = crate::secrets::StaticSecrets::default();
        let c = cfg(&s, &sec).unwrap();
        let h = health(&c, &s).unwrap();
        assert_eq!(h["version"], "4.2.3");
        assert_eq!(h["tiers"], json!(["basic", "advanced"]));
        let job = Job { bytes: b"%PDF-1.4 test", sha: "ab", ext: "PDF", range: Some("1") };
        let cancel = AtomicBool::new(false);
        let noop = |_: &str| {};
        let v = parse_tier(&c, &s, &job, "advanced", "4.2.3", &cancel, &noop).unwrap();
        assert_eq!(v, json!({"0": [{"type": "text", "content": "advanced 文字", "bbox": null}]}));
        let v = parse_tier(&c, &s, &job, "basic", "4.2.3", &cancel, &noop).unwrap();
        assert_eq!(v["0"][0]["content"], "basic 文字");
        assert!(hits.lock().unwrap().iter().any(|l| l.starts_with("PUT /v1/tasks/job_basic/uploads/f_basic")));
        // cache file round trip
        let dir = std::env::temp_dir().join(format!("dp-ocr-{}", crate::text::short_id(6)));
        let cp = cache_path(&dir, "ab", "basic", Some("1"), "4.2.3", &base);
        cache_put(&cp, &v);
        assert_eq!(std::fs::read_to_string(&cp).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()), Some(v));
        assert_ne!(cp, cache_path(&dir, "ab", "advanced", Some("1"), "4.2.3", &base));
        let _ = std::fs::remove_dir_all(dir);
        // unreachable server → error text names the server
        s.doc_parse.server = "http://127.0.0.1:1".into();
        let e = health(&cfg(&s, &sec).unwrap(), &s).unwrap_err();
        assert!(e.contains("连接失败"), "{e}");
    }
}
