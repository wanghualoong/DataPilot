//! web_fetch (port of Swift WebFetchTool without the Chrome `render` path): proxy-aware GET, HTML → text,
//! HTML tables saved as CSV to uploads/, direct data files downloaded to uploads/.
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::{imp, sv, Impl, ToolCtx, ToolResult};
use crate::text;

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("web_fetch", imp(web_fetch, |a| format!("访问 {}", sv(a, "url"))));
    m.insert("web_search", imp(web_search, |a| format!("搜索 {}", sv(a, "query"))));
}

/// Local Chrome (system install or the app-managed Chrome for Testing) + proxy flags from the settings.
pub fn chrome(c: &ToolCtx) -> Option<(std::path::PathBuf, Vec<String>)> {
    crate::chrome::find(&c.shared.paths.app_support).map(|e| (e, crate::chrome::proxy_args(&c.shared.settings)))
}

/// Chrome for web_search: when none is installed, ask once (approval card, kind `chrome_install`) to download
/// Chrome for Testing from Google's official bucket. Denied / failed → None (caller falls back to Bing).
pub(crate) fn chrome_or_install(c: &ToolCtx, notes: &mut Vec<String>) -> Option<(std::path::PathBuf, Vec<String>)> {
    if let Some(x) = chrome(c) { return Some(x); }
    let id = format!("chrome_{}", text::short_id(8));
    (c.emit)(crate::events::approval_required(&id, "web_search", "下载 Chrome for Testing（约 150 MB，仅首次）", "network", &serde_json::json!({}),
                                              "chrome_install", "storage.googleapis.com"));
    let (choice, _) = c.shared.broker.wait_approval(&id);
    if matches!(choice, crate::permissions::Choice::Deny) { notes.push("未安装 Chrome（用户暂不下载），改用 Bing".into()); return None; }
    let agent = match download_agent(c, crate::chrome::CFT_INDEX) { Ok(a) => a, Err(e) => { notes.push(format!("Chrome 安装失败：{e}")); return None; } };
    let progress = |m: &str| (c.emit)(crate::events::tool_progress(&c.call_id, m));
    match crate::chrome::install(&c.shared.paths.app_support, &agent, &progress) {
        Ok(exe) => {
            (c.emit)(crate::events::notice("✅ Chrome for Testing 已安装，之后联网搜索会使用 Google。"));
            Some((exe, crate::chrome::proxy_args(&c.shared.settings)))
        }
        Err(e) => {
            (c.emit)(crate::events::notice(&format!("⚠️ Chrome 安装失败：{e}。本次改用 Bing；也可以手动安装 Google Chrome。")));
            notes.push(format!("Chrome 安装失败：{e}"));
            None
        }
    }
}

/// Large downloads: connect / per-read timeouts instead of one overall deadline; proxy as for web_fetch.
fn download_agent(c: &ToolCtx, url: &str) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let mut b = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).timeout_connect(Duration::from_secs(30)).timeout_read(Duration::from_secs(60));
    if let Some(p) = proxy_for(c, url) { if let Ok(px) = ureq::Proxy::new(&p) { b = b.proxy(px); } }
    Ok(b.build())
}

fn agent_for(c: &ToolCtx, url: &str, secs: u64) -> Result<(ureq::Agent, Option<String>), String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let mut b = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).timeout(Duration::from_secs(secs));
    let mut note = None;
    if let Some(p) = proxy_for(c, url) {
        if let Ok(px) = ureq::Proxy::new(&p) { b = b.proxy(px); note = Some(format!("经代理 {p}")); }
    }
    Ok((b.build(), note))
}

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

// ---------- web_search: Google in the local Chrome, Bing HTML fallback (port of Swift WebSearch) ----------
#[derive(Clone, Debug, PartialEq)]
pub struct Hit { pub title: String, pub url: String, pub site: String, pub snippet: String }

const GOOGLE_JS: &str = r#"(() => {
  const out = [], seen = new Set();
  document.querySelectorAll('a h3').forEach(h => {
    const a = h.closest('a'); if (!a) return;
    let u = a.getAttribute('href') || '';
    try { const x = new URL(u, location.href); u = x.href;
          if (x.pathname === '/url') u = x.searchParams.get('q') || x.searchParams.get('url') || u; } catch (e) {}
    if (seen.has(u)) return; seen.add(u);
    const box = a.closest('.MjjYud, .g, [data-hveid]') || a.parentElement;
    const cite = box && box.querySelector('cite');
    const sn = box && (box.querySelector('.VwiC3b, [data-sncf], .IsZvec'));
    out.push({ t: h.innerText.trim(), u, c: cite ? cite.innerText.split(' ›')[0].trim() : '', s: sn ? sn.innerText.trim() : '' });
  });
  return JSON.stringify({ captcha: /\/sorry\//.test(location.href) || /unusual traffic|recaptcha/i.test(document.body ? document.body.innerText.slice(0, 4000) : ''), items: out });
})()"#;

pub fn parse_bing(html: &str) -> Vec<Hit> {
    let h2 = re(r#"<h2[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>"#);
    let p = re(r"<p[^>]*>([\s\S]*?)</p>");
    let tag = re(r"<[^>]+>");
    let lead = re(r"^\s*\S*网页\s*");
    html.split("class=\"b_algo\"").skip(1).filter_map(|b| {
        let m = h2.captures(b)?;
        let url = decode(&m[1]);
        let title = decode(&tag.replace_all(&m[2], "")).trim().to_string();
        let snippet = p.captures(b).map(|c| lead.replace(&decode(&tag.replace_all(&c[1], "")), "").trim().to_string()).unwrap_or_default();
        let site = url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string();
        Some(Hit { title, url, site, snippet: text::prefix(&snippet, 300) })
    }).collect()
}

fn google(q: &str, num: usize, exe: &std::path::Path, c: &ToolCtx, proxy: &[String]) -> Result<(bool, Vec<Hit>), String> {
    let url = format!("https://www.google.com/search?q={}&hl=zh-CN&num={}", crate::chrome::qenc(q), num.max(10));
    let ready = "document.querySelectorAll('a h3').length > 0 || /\\/sorry\\//.test(location.href) || /unusual traffic/i.test(document.body ? document.body.innerText.slice(0, 4000) : '')";
    let v = crate::chrome::evaluate(&url, exe, &c.shared.paths.app_support, proxy, ready, GOOGLE_JS, Duration::from_secs(15))?;
    let j: Value = serde_json::from_str(v.as_str().unwrap_or("{}")).map_err(|e| e.to_string())?;
    let items = j["items"].as_array().into_iter().flatten().filter_map(|i| {
        let t = i["t"].as_str().filter(|t| !t.is_empty())?;
        Some(Hit { title: t.into(), url: i["u"].as_str()?.into(), site: sv(i, "c"), snippet: text::prefix(&sv(i, "s"), 300) })
    }).take(num).collect();
    Ok((j["captcha"] == true, items))
}

fn bing(q: &str, num: usize, c: &ToolCtx) -> Result<Vec<Hit>, String> {
    let url = format!("https://cn.bing.com/search?q={}&count={num}", crate::chrome::qenc(q));
    let (agent, _) = agent_for(c, &url, 15)?;
    let html = agent.get(&url).set("User-Agent", UA).set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8").call()
        .map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
    Ok(parse_bing(&html).into_iter().take(num).collect())
}

/// Per-process search cache (15 min) + per-turn call budget (same limits as the Swift core).
struct SearchCache { entries: HashMap<String, (std::time::Instant, &'static str, Vec<Hit>)>, per_turn: HashMap<String, usize> }
fn cache() -> &'static std::sync::Mutex<SearchCache> {
    static C: OnceLock<std::sync::Mutex<SearchCache>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(SearchCache { entries: HashMap::new(), per_turn: HashMap::new() }))
}

fn web_search(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let q = sv(a, "query").trim().to_string();
    if q.is_empty() { return Ok(ToolResult::err("query is empty")); }
    if !c.shared.network() { return Ok(ToolResult::err("联网已关闭 / network is off")); }
    let num = a["num"].as_u64().unwrap_or(8).clamp(1, 10) as usize;
    let key = format!("{q}|{num}");
    let cached = {
        let mut g = cache().lock().unwrap();
        let n = g.per_turn.entry(c.turn_id.clone()).or_insert(0);
        *n += 1;
        if *n > 8 { return Ok(ToolResult::err("本轮搜索次数已达上限（8 次）。请基于已有结果回答，或用 web_fetch 读取已找到的页面。")); }
        g.entries.get(&key).filter(|e| e.0.elapsed() < Duration::from_secs(900)).map(|e| (e.1, e.2.clone()))
    };
    let mut notes: Vec<String> = vec![];
    let mut backend = "google";
    let mut hits: Option<Vec<Hit>> = None;
    let from_cache = cached.is_some();
    if let Some((b, h)) = cached { backend = b; hits = Some(h); }
    else { match chrome_or_install(c, &mut notes) {
        Some((exe, proxy)) => {
            if crate::chrome::verifying() {
                notes.push("Google 人机验证窗口仍打开（完成验证并关闭该窗口后恢复使用 Google），本次改用 Bing".into());
            } else {
            (c.emit)(crate::events::tool_progress(&c.call_id, &format!("Chrome → Google: {q}")));
            match google(&q, num, &exe, c, &proxy) {
                Ok((true, _)) => {
                    let url = format!("https://www.google.com/search?q={}&hl=zh-CN", crate::chrome::qenc(&q));
                    match crate::chrome::open_visible(&url, &exe, &c.shared.paths.app_support, &proxy) {
                        Ok(()) => {
                            (c.emit)(crate::events::notice("🔐 Google 要求人机验证：已打开一个 Chrome 窗口，请在其中完成一次验证后关闭该窗口，之后搜索会自动恢复使用 Google。本次先用 Bing。"));
                            notes.push("Google 要求人机验证：已为用户打开 Chrome 窗口完成验证；本次改用 Bing".into());
                        }
                        Err(e) => notes.push(format!("Google 要求人机验证，打开验证窗口失败（{e}）；本次改用 Bing")),
                    }
                }
                Ok((false, h)) if !h.is_empty() => hits = Some(h),
                Ok(_) => notes.push("Google 没有返回结果".into()),
                Err(e) => notes.push(format!("Google 搜索失败：{e}")),
            }
            }
        }
        None => { if notes.is_empty() { notes.push("未找到本机 Chrome，改用 Bing".into()); } }
    } }
    if hits.is_none() {
        backend = "bing";
        (c.emit)(crate::events::tool_progress(&c.call_id, &format!("Bing: {q}")));
        match bing(&q, num, c) {
            Ok(h) => hits = Some(h),
            Err(e) => { notes.push(format!("Bing 搜索失败：{e}")); return Ok(ToolResult::err(format!("搜索失败：{}。请如实告诉用户具体原因。", notes.join("；")))); }
        }
    }
    let hits = hits.unwrap_or_default();
    if hits.is_empty() { return Ok(ToolResult::ok(format!("没有搜索到结果（{}）。可以换个关键词再试。", notes.join("；")))); }
    if !from_cache {
        let mut g = cache().lock().unwrap();
        if g.entries.len() > 200 { g.entries.clear(); }
        g.entries.insert(key, (std::time::Instant::now(), backend, hits.clone()));
    }
    let mut lines = vec![format!("搜索「{q}」（{}，{} 条）：", if backend == "google" { "Google via Chrome" } else { "Bing" }, hits.len())];
    for (i, h) in hits.iter().enumerate() {
        lines.push(format!("{}. {}\n   {}{}", i + 1, h.title, h.url, if h.snippet.is_empty() { String::new() } else { format!("\n   {}", h.snippet) }));
    }
    if !notes.is_empty() && backend == "bing" { lines.push(format!("({})", notes.join("；"))); }
    if from_cache { lines.push("(缓存结果)".into()); }
    let text = format!("<untrusted_web_content>\n{}\n</untrusted_web_content>\n下一步：挑 2–3 个最相关的结果，并行调用 web_fetch 读取原文后再回答，并在回答中注明来源链接。", lines.join("\n"));
    let items: Vec<Value> = hits.iter().map(|h| serde_json::json!({"title": h.title, "url": h.url, "site": h.site, "snippet": h.snippet})).collect();
    Ok(ToolResult::with(text, serde_json::json!({"kind": "search", "query": q, "backend": backend, "results": items})))
}

fn re(p: &str) -> regex::Regex { regex::RegexBuilder::new(p).case_insensitive(true).build().unwrap() }

fn decode(s: &str) -> String {
    let mut t = s.to_string();
    for (k, v) in [("&nbsp;", " "), ("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'")] { t = t.replace(k, v); }
    t
}

pub fn html_to_text(html: &str) -> String {
    static R: OnceLock<[regex::Regex; 6]> = OnceLock::new();
    let r = R.get_or_init(|| [
        re(r"<(script|style|noscript|svg|head)[^>]*>[\s\S]*?</(script|style|noscript|svg|head)>"),
        re(r"<(br|/p|/div|/li|/tr|/h[1-6])[^>]*>"),
        re(r"<h[1-6][^>]*>"),
        re(r"<[^>]+>"),
        re(r"[ \t]+"),
        re(r"\n\s*\n+"),
    ]);
    let s = r[0].replace_all(html, " ");
    let s = r[1].replace_all(&s, "\n");
    let s = r[2].replace_all(&s, "\n## ");
    let s = r[3].replace_all(&s, " ");
    let s = decode(&s);
    let s = r[4].replace_all(&s, " ");
    r[5].replace_all(&s, "\n\n").trim().to_string()
}

pub fn html_tables(html: &str) -> Vec<Vec<Vec<String>>> {
    let (t, r, c, tag) = (re(r"<table[^>]*>([\s\S]*?)</table>"), re(r"<tr[^>]*>([\s\S]*?)</tr>"), re(r"<t[hd][^>]*>([\s\S]*?)</t[hd]>"), re(r"<[^>]+>"));
    t.captures_iter(html).map(|tm| {
        r.captures_iter(&tm[1]).map(|rm| c.captures_iter(&rm[1]).map(|cm| decode(&tag.replace_all(&cm[1], " ")).trim().to_string()).collect::<Vec<_>>())
            .filter(|row| !row.is_empty()).collect()
    }).collect()
}

fn proxy_for(c: &ToolCtx, url: &str) -> Option<String> {
    let s = &c.shared.settings;
    match s.proxy_mode.as_str() {
        "custom" if !s.proxy_host.is_empty() && s.proxy_port > 0 => Some(format!("http://{}:{}", s.proxy_host, s.proxy_port)),
        "system" => crate::llm::system_proxy_for(url),
        _ => None,
    }
}

fn web_fetch(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let url = sv(a, "url");
    if !(url.starts_with("http://") || url.starts_with("https://")) { return Ok(ToolResult::err("invalid url")); }
    if !c.shared.network() { return Ok(ToolResult::err("联网已关闭 / network is off")); }
    if a["render"] == true {
        let Some((exe, proxy)) = chrome(c) else { return Ok(ToolResult::err("无法用 Chrome 渲染：未找到本机 Chrome。可去掉 render 参数直接抓取。")) };
        return match crate::chrome::render(&url, &exe, &c.shared.paths.app_support, &proxy, "document.readyState === 'complete'", Duration::from_secs(20)) {
            Ok((html, fin)) => Ok(page(&html, 200, &fin, a, c, Some("Chrome 渲染".into()))),
            Err(e) => Ok(ToolResult::err(format!("Chrome 渲染失败：{e}"))),
        };
    }
    let (agent, note) = agent_for(c, &url, 30)?;
    let host = crate::permissions::host(a).unwrap_or_else(|| url.clone());
    let resp = match agent.get(&url)
        .set("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 DataPilot/0.1")
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8").call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Ok(ToolResult::err(format!("无法访问 {host}：{e}。这是具体的网络错误，请如实告诉用户，可换一个来源再试。"))),
    };
    let code = resp.status();
    let ctype = resp.header("Content-Type").unwrap_or("").to_lowercase();
    let mut data = Vec::new();
    resp.into_reader().take(50_000_000).read_to_end(&mut data).map_err(|e| e.to_string())?;
    let fname = url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
    let ext = fname.rsplit_once('.').map(|x| x.1.to_lowercase()).unwrap_or_default();
    if ["csv", "xlsx", "xls", "json", "parquet", "tsv"].contains(&ext.as_str()) {
        let dest = c.workspace().join("uploads").join(&fname);
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&dest, &data).map_err(|e| e.to_string())?;
        return Ok(ToolResult::ok(format!("HTTP {code}，已下载数据文件到 uploads/{fname}（{} bytes）。用 data_import 导入。", data.len())));
    }
    let html = String::from_utf8_lossy(&data).to_string();
    if !ctype.contains("html") && !html.contains("<html") { return Ok(ToolResult::ok(format!("HTTP {code}\n{}", text::truncate_middle(&html, 20000)))); }
    Ok(page(&html, code, &url, a, c, note))
}

/// HTML → text (+ tables saved as CSV). Shared by plain fetch and Chrome-rendered fetch.
fn page(html: &str, code: u16, url: &str, a: &Value, c: &ToolCtx, note: Option<String>) -> ToolResult {
    let mut saved = vec![];
    if a["save_tables"] != false {
        for (i, t) in html_tables(html).into_iter().take(10).enumerate().filter(|(_, t)| t.len() >= 2) {
            let csv = t.iter().map(|r| r.iter().map(|x| format!("\"{}\"", x.replace('"', "\"\""))).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n");
            let name = format!("uploads/web_table_{}_{}.csv", text::short_id(4), i + 1);
            let _ = std::fs::create_dir_all(c.workspace().join("uploads"));
            if std::fs::write(c.workspace().join(&name), csv).is_ok() { saved.push(format!("{name}（{} 行 × {} 列）", t.len(), t[0].len())); }
        }
    }
    let body = html_to_text(html);
    let mut out = format!("HTTP {code} {url}\n<untrusted_web_content>\n{}\n</untrusted_web_content>", text::truncate_middle(&body, 15000));
    if !saved.is_empty() { out += &format!("\n页面表格已保存：\n{}", saved.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")); }
    if let Some(n) = note { out += &format!("\n({n})"); }
    if body.chars().count() < 300 && a["render"] != true { out += "\n(正文很少，页面可能依赖 JavaScript；可以用 render:true 让 Chrome 渲染后再抓取。)"; }
    ToolResult::ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html() {
        let h = "<html><head><title>x</title></head><body><h2>标题</h2><p>a &amp; b</p><script>evil()</script><table><tr><th>区</th><th>额</th></tr><tr><td>华东</td><td>1</td></tr></table></body></html>";
        let t = html_to_text(h);
        assert!(t.contains("## 标题") && t.contains("a & b") && !t.contains("evil"));
        let b = r#"<li class="b_algo"><h2><a href="https://a.com/x">A &amp; B</a></h2><p>网页 摘要</p></li>"#;
        assert_eq!(parse_bing(b), vec![Hit { title: "A & B".into(), url: "https://a.com/x".into(), site: "a.com".into(), snippet: "摘要".into() }]);
        assert_eq!(html_tables(h)[0], vec![vec!["区".to_string(), "额".into()], vec!["华东".into(), "1".into()]]);
    }
}
