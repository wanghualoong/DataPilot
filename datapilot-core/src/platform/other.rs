//! Other Unix hosts (CI / Linux dev): portable fallbacks.
//! are completed in phase C; until then they degrade safely (direct connection, UTC, error message).
use std::path::{Path, PathBuf};

pub fn app_support_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| super::home().join(".local/share")).join("DataPilot")
}
pub fn python_exe(root: &Path) -> PathBuf { root.join("bin/python3") }
pub fn ssh_exe() -> PathBuf {
    PathBuf::from("/usr/bin/ssh")
}
pub fn system_proxy_for(_url: &str) -> Option<String> { None }   // phase C: WinHttpGetIEProxyConfigForCurrentUser + PAC
pub fn utc_offset() -> i64 { 0 }                                  // phase C: GetTimeZoneInformation
pub fn move_to_trash(p: &Path) -> Result<(), String> {             // phase C: IFileOperation / SHFileOperationW
    Err(format!("回收站功能尚未实现：{}", p.display()))
}
pub fn copy_tree(src: &Path, dest: &Path) -> bool { super::copy_tree_plain(src, dest).is_ok() }

/// `df -k` (POSIX output): available 1K blocks of the volume holding `p`.
pub fn free_space(p: &Path) -> Option<u64> {
    let o = std::process::Command::new("/bin/df").arg("-Pk").arg(p).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout);
    t.lines().nth(1)?.split_whitespace().nth(3)?.parse::<u64>().ok().map(|k| k * 1024)
}
