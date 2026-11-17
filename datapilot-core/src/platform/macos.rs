//! macOS implementations.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

pub fn app_support_dir() -> PathBuf { super::home().join("Library/Application Support/DataPilot") }
pub fn python_exe(root: &Path) -> PathBuf { root.join("bin/python3") }
pub fn ssh_exe() -> PathBuf { PathBuf::from("/usr/bin/ssh") }

fn scutil() -> String {
    Command::new("/usr/sbin/scutil").arg("--proxy").output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}
fn scutil_get(s: &str, k: &str) -> Option<String> {
    s.lines().find_map(|l| { let l = l.trim(); l.strip_prefix(k).and_then(|r| r.trim().strip_prefix(':')).map(|v| v.trim().to_string()) })
}

pub fn system_proxy_for(url: &str) -> Option<String> {
    let s = scutil();
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':', '?']).next().unwrap_or("").to_string();
    if scutil_get(&s, "ProxyAutoConfigEnable").as_deref() == Some("1") {
        if let Some(pac_url) = scutil_get(&s, "ProxyAutoConfigURLString") {
            let pac = if let Some(p) = pac_url.strip_prefix("file://") { std::fs::read_to_string(p).ok() } else {
                ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(5)).build().get(&pac_url).call().ok().and_then(|r| r.into_string().ok())
            };
            if let Some(r) = pac.as_deref().and_then(|p| crate::llm::eval_pac(p, &host)) { return r; }
        }
    }
    if scutil_get(&s, "HTTPSEnable").as_deref() != Some("1") { return None; }
    let excluded = s.split("ExceptionsList").nth(1).map(|rest| {
        rest.lines().skip(1).take_while(|l| !l.trim().starts_with('}')).filter_map(|l| l.split_once(':').map(|x| x.1.trim().to_string()))
            .any(|e| crate::llm::sh_exp_match(&host, &e) || host.ends_with(e.trim_start_matches('*')))
    }).unwrap_or(false);
    if excluded { return None; }
    Some(format!("http://{}:{}", scutil_get(&s, "HTTPSProxy")?, scutil_get(&s, "HTTPSPort")?))
}

pub fn utc_offset() -> i64 {
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        let out = Command::new("/bin/date").arg("+%z").output().ok();
        let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if s.len() == 5 {
            let sign = if s.starts_with('-') { -1 } else { 1 };
            sign * (s[1..3].parse::<i64>().unwrap_or(0) * 3600 + s[3..5].parse::<i64>().unwrap_or(0) * 60)
        } else { 0 }
    })
}

pub fn move_to_trash(p: &Path) -> Result<(), String> {
    let ok = Command::new("/usr/bin/trash").arg(p).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if ok { return Ok(()); }
    let script = format!("tell application \"Finder\" to delete POSIX file \"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
    let ok = Command::new("/usr/bin/osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err(format!("无法移到废纸篓：{}", p.display())) }
}

pub fn copy_tree(src: &Path, dest: &Path) -> bool {
    let ok = Command::new("/bin/cp").args(["-c", "-R"]).arg(src).arg(dest).stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    ok || super::copy_tree_plain(src, dest).is_ok()
}

/// `df -k` (POSIX output): available 1K blocks of the volume holding `p`.
pub fn free_space(p: &Path) -> Option<u64> {
    let o = std::process::Command::new("/bin/df").arg("-Pk").arg(p).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout);
    t.lines().nth(1)?.split_whitespace().nth(3)?.parse::<u64>().ok().map(|k| k * 1024)
}
