//! Platform adapter (ADR-0012 L3): everything that differs between macOS and Windows lives behind these functions.
//! macOS is the reference implementation; Windows is filled in by phase C of M3.
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")] mod macos;
#[cfg(target_os = "macos")] use macos as imp;
#[cfg(windows)] mod windows;
#[cfg(windows)] use windows as imp;
#[cfg(not(any(target_os = "macos", windows)))] mod other;
#[cfg(not(any(target_os = "macos", windows)))] use other as imp;

pub fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default()) }

/// Default app data folder (macOS `~/Library/Application Support/DataPilot`, Windows `%APPDATA%\DataPilot`).
pub fn app_support_dir() -> PathBuf { imp::app_support_dir() }

/// The bundled interpreter inside `python_root` (`bin/python3` / `python.exe`).
pub fn python_exe(python_root: &Path) -> PathBuf { imp::python_exe(python_root) }

/// OpenSSH client — a fixed system path, never resolved through PATH.
pub fn ssh_exe() -> PathBuf { imp::ssh_exe() }

/// System proxy for a URL (macOS: scutil incl. PAC; Windows: WinHTTP / IE settings). None = direct.
pub fn system_proxy_for(url: &str) -> Option<String> { imp::system_proxy_for(url) }

/// Local UTC offset in seconds.
pub fn utc_offset() -> i64 { imp::utc_offset() }

/// `std::fs::canonicalize` without the Windows `\\?\` verbatim prefix (which many tools and APIs reject).
pub fn canonicalize(p: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") { return Ok(PathBuf::from(format!(r"\\{rest}"))); }
        if let Some(rest) = s.strip_prefix(r"\\?\") { return Ok(PathBuf::from(rest)); }
    }
    Ok(c)
}

/// Free bytes on the volume holding `p` (None when unknown).
pub fn free_space(p: &Path) -> Option<u64> { imp::free_space(p) }

/// Move a file / folder to the Trash / Recycle Bin.
pub fn move_to_trash(p: &Path) -> Result<(), String> { imp::move_to_trash(p) }

/// Copy a directory tree (macOS: APFS clone when possible). `dest` must not exist.
pub fn copy_tree(src: &Path, dest: &Path) -> bool { imp::copy_tree(src, dest) }

/// Mark a folder hidden like a dot-folder on macOS (Windows: FILE_ATTRIBUTE_HIDDEN, as Git does for .git). No-op elsewhere.
pub fn hide(p: &Path) {
    #[cfg(windows)]
    imp::hide(p);
    #[cfg(not(windows))]
    let _ = p;
}

/// Windows: tie all child processes to the core's lifetime (Job Object). No-op elsewhere.
pub fn contain_children() {
    #[cfg(windows)]
    imp::contain_children();
}

/// Portable recursive copy (fallback for every platform).
pub fn copy_tree_plain(src: &Path, dest: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(src)?;
    if m.file_type().is_symlink() { return Ok(()); }   // never follow links out of the tree
    if m.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? { let e = e?; copy_tree_plain(&e.path(), &dest.join(e.file_name()))?; }
    } else {
        std::fs::copy(src, dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_and_paths() {
        let d = std::env::temp_dir().join(format!("dpplat-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/x.txt"), "1").unwrap();
        assert!(copy_tree(&d.join("a"), &d.join("c")));
        assert_eq!(std::fs::read_to_string(d.join("c/b/x.txt")).unwrap(), "1");
        copy_tree_plain(&d.join("a"), &d.join("e")).unwrap();
        assert!(d.join("e/b/x.txt").exists());
        assert!(python_exe(Path::new("/py")).starts_with("/py"));
        assert!(ssh_exe().is_absolute());
        assert!(utc_offset().abs() <= 14 * 3600);
        let _ = std::fs::remove_dir_all(&d);
    }
}
