//! Diagnostic log (0.1.9): `AppSupport/logs/datapilot.log`, one line per event — turns, model calls, tools, kernel,
//! SSH tunnels, document parsing. Never conversation text, data values or secrets: names, durations, counts, status
//! and (redacted, ≤ 1000 chars) error messages only.
//! Rotation: a new file at 5 MB, at most 5 files, nothing older than 14 days (checked at start and on every rotation).
//! Line format: `2026-11-23 17:01:02.123 WARN  turn_end s=<session> reason=error ms=81234 llm=3 tools=5 slow=1`.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_FILES: usize = 5;
pub const KEEP_DAYS: f64 = 14.0;
/// "slow" thresholds: whole turn, wait for the first token, one tool call
pub const SLOW_TURN_MS: u128 = 180_000;
pub const SLOW_TTFT_MS: u128 = 30_000;
pub const SLOW_TOOL_MS: u128 = 120_000;
const NAME: &str = "datapilot.log";

pub struct Logger { dir: PathBuf, file: Option<File>, size: u64, max_bytes: u64 }

static LOG: Mutex<Option<Logger>> = Mutex::new(None);

pub fn dir(app_support: &Path) -> PathBuf { app_support.join("logs") }

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Rotated files (`datapilot-*.log`) with their modification time.
fn rotated(dir: &Path) -> Vec<(f64, PathBuf)> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with("datapilot-") && n.ends_with(".log")).unwrap_or(false))
        .map(|p| (mtime(&p), p)).collect()
}

impl Logger {
    pub fn open(dir: &Path, max_bytes: u64) -> Logger {
        let _ = std::fs::create_dir_all(dir);
        let mut l = Logger { dir: dir.to_path_buf(), file: None, size: 0, max_bytes };
        l.prune();
        l.reopen();
        l
    }
    fn reopen(&mut self) {
        let p = self.dir.join(NAME);
        self.file = OpenOptions::new().create(true).append(true).open(&p).ok();
        self.size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    }
    fn rotate(&mut self) {
        self.file = None;
        let (y, m, d, hh, mm, ss, _) = crate::text::local_time(crate::text::now());
        let base = format!("datapilot-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}");
        let mut dst = self.dir.join(format!("{base}.log"));
        let mut i = 1;
        while dst.exists() { dst = self.dir.join(format!("{base}-{i}.log")); i += 1; }
        let _ = std::fs::rename(self.dir.join(NAME), dst);
        self.prune();
        self.reopen();
    }
    /// Keep the newest MAX_FILES - 1 rotated files (the live file is the last one) and none older than KEEP_DAYS.
    fn prune(&self) {
        let mut old = rotated(&self.dir);
        old.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let now = crate::text::now();
        for (i, (t, p)) in old.iter().enumerate() {
            if i + 1 >= MAX_FILES || now - t > KEEP_DAYS * 86400.0 { let _ = std::fs::remove_file(p); }
        }
    }
    pub fn write_line(&mut self, line: &str) {
        let n = line.len() as u64 + 1;
        if self.size > 0 && self.size + n > self.max_bytes { self.rotate(); }
        if let Some(f) = self.file.as_mut() { if writeln!(f, "{line}").is_ok() { self.size += n; } }
    }
}

/// Start logging (the core binary calls this once). Also expires the previous generation of core.log.
pub fn init(app_support: &Path) {
    *LOG.lock().unwrap() = Some(Logger::open(&dir(app_support), MAX_BYTES));
    let old = app_support.join("core.log.1");
    if old.exists() && crate::text::now() - mtime(&old) > KEEP_DAYS * 86400.0 { let _ = std::fs::remove_file(old); }
}

fn quote(v: &str) -> String {
    let v = crate::memory::redact(v).replace(['\n', '\r', '\t'], " ");
    if v.is_empty() { "\"\"".into() } else if v.contains([' ', '"', '=']) { format!("\"{}\"", v.replace('"', "'")) } else { v }
}

pub fn fmt_line(level: &str, event: &str, session: &str, fields: &[(&str, String)]) -> String {
    let now = crate::text::now();
    let (y, m, d, hh, mm, ss, _) = crate::text::local_time(now);
    let ms = (now.fract() * 1000.0) as u32;
    let mut s = format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{ms:03} {level:<5} {event}");
    if !session.is_empty() { s += &format!(" s={session}"); }
    for (k, v) in fields { s += &format!(" {k}={}", quote(v)); }
    s
}

/// One event line. No-op until `init` (unit tests, tools run outside the app).
pub fn log(level: &str, event: &str, session: &str, fields: &[(&str, String)]) {
    let mut g = LOG.lock().unwrap();
    if let Some(l) = g.as_mut() { l.write_line(&fmt_line(level, event, session, fields)); }
}
pub fn info(event: &str, session: &str, fields: &[(&str, String)]) { log("INFO", event, session, fields) }
pub fn warn(event: &str, session: &str, fields: &[(&str, String)]) { log("WARN", event, session, fields) }
pub fn error(event: &str, session: &str, fields: &[(&str, String)]) { log("ERROR", event, session, fields) }

/// Error text for a log field: redacted later by `quote`, cut here.
pub fn err_text(e: &str) -> String { crate::text::prefix(e.trim(), 1000) }

/// The last `n` WARN / ERROR lines (oldest first) of the rotated and live files — /doctor shows them.
pub fn recent_problems(app_support: &Path, n: usize) -> Vec<String> {
    let d = dir(app_support);
    let mut files = rotated(&d);
    files.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    files.push((0.0, d.join(NAME)));
    let mut out: Vec<String> = vec![];
    for (_, f) in files {
        if let Ok(t) = std::fs::read_to_string(&f) {
            out.extend(t.lines().filter(|l| l.contains(" WARN ") || l.contains(" ERROR ")).map(String::from));
        }
    }
    let k = out.len().saturating_sub(n);
    out.split_off(k)
}

/// 诊断包: every log file (logs/*.log, core.log, core.log.1) + `info.txt` (version, OS, /doctor result) as a zip.
/// No chats, memory, data or config: the logs themselves never hold those.
pub fn diagnostics_zip(app_support: &Path, dst: &Path, doctor: &serde_json::Value) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    let d = dir(app_support);
    let mut files: Vec<(String, PathBuf)> = rotated(&d).into_iter().map(|(_, p)| p).chain([d.join(NAME)])
        .filter(|p| p.is_file()).map(|p| (format!("logs/{}", p.file_name().unwrap().to_string_lossy()), p)).collect();
    for n in ["core.log", "core.log.1"] { let p = app_support.join(n); if p.is_file() { files.push((n.to_string(), p)); } }
    files.sort();
    if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
    let tmp = dst.with_extension("zip.part");
    let r = (|| -> Result<(), String> {
        let f = File::create(&tmp).map_err(|e| e.to_string())?;
        let mut w = zip::ZipWriter::new(f);
        let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut info = format!("DataPilot {} · {} · {}\n生成时间 / created: {}\n\n", crate::VERSION, std::env::consts::OS, std::env::consts::ARCH,
                               fmt_line("", "", "", &[]).trim());
        for it in doctor["items"].as_array().into_iter().flatten() {
            let ok = match it["ok"].as_bool() { Some(true) => "OK  ", Some(false) => "FAIL", None => "WARN" };
            info += &format!("[{ok}] {}: {}\n", it["key"].as_str().unwrap_or(""), crate::memory::redact(it["detail"].as_str().unwrap_or("")));
        }
        w.start_file("info.txt", o).map_err(|e| e.to_string())?;
        w.write_all(info.as_bytes()).map_err(|e| e.to_string())?;
        for (name, p) in &files {
            w.start_file(name.as_str(), o).map_err(|e| e.to_string())?;
            let mut src = File::open(p).map_err(|e| e.to_string())?;
            std::io::copy(&mut src, &mut w).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    match r {
        Ok(()) => std::fs::rename(&tmp, dst).map_err(|e| e.to_string()),
        Err(e) => { let _ = std::fs::remove_file(&tmp); Err(e) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_retention() {
        let d = std::env::temp_dir().join(format!("dp-log-{}", crate::text::short_id(6)));
        // a rotated file from 20 days ago is removed at open
        std::fs::create_dir_all(&d).unwrap();
        let stale = d.join("datapilot-20200101-000000.log");
        std::fs::write(&stale, "x\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(20 * 86400);
        File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
        let mut l = Logger::open(&d, 300);
        assert!(!stale.exists());
        for i in 0..200 { l.write_line(&fmt_line("INFO", "tool", "s1", &[("name", format!("python{i}")), ("ms", "12".into())])); }
        let files = rotated(&d);
        assert!(!files.is_empty() && files.len() < MAX_FILES, "rotated files: {}", files.len());
        assert!(std::fs::metadata(d.join(NAME)).unwrap().len() <= 300);
    }

    #[test]
    fn diagnostics_bundle() {
        let sup = std::env::temp_dir().join(format!("dp-diag-{}", crate::text::short_id(6)));
        let mut l = Logger::open(&dir(&sup), MAX_BYTES);
        l.write_line(&fmt_line("WARN", "tool", "s1", &[("name", "python".into()), ("ok", "0".into())]));
        drop(l);
        std::fs::write(sup.join("core.log"), "stderr line\n").unwrap();
        std::fs::write(sup.join("config.json"), "{\"secret\": 1}").unwrap();
        let dst = sup.join("out/diag.zip");
        diagnostics_zip(&sup, &dst, &serde_json::json!({"items": [{"key": "core", "ok": true, "detail": "x"}]})).unwrap();
        let mut z = zip::ZipArchive::new(File::open(&dst).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"info.txt".into()) && names.contains(&"logs/datapilot.log".into()) && names.contains(&"core.log".into()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("config")));
        assert_eq!(recent_problems(&sup, 10).len(), 1);
        let _ = std::fs::remove_dir_all(&sup);
    }

    #[test]
    fn redacts_and_flattens() {
        let l = fmt_line("WARN", "tool", "s", &[("err", "bad api_key=sk-abcdefghijklmnopqrstuvwxyz\nline2".into())]);
        assert!(!l.contains("sk-abcdef") && !l.contains('\n') && l.contains("REDACTED"), "{l}");
        assert!(l.contains(" WARN  tool s=s err=\""));
    }
}
