//! Per-turn workspace snapshots for non-project sessions (port of Swift `WorkspaceSnapshots`): APFS clones (`cp -c`)
//! in <session>/snapshots/<turn_id>, newest 20 kept; `session/rewind` restores one (kernel stopped first).
use std::path::{Path, PathBuf};

pub const KEEP: usize = 20;

fn dir(session_dir: &Path) -> PathBuf { session_dir.join("snapshots") }

fn clone_tree(src: &Path, dest: &Path) -> bool { crate::platform::copy_tree(src, dest) }

pub fn take(session_dir: &Path, workspace: &Path, turn_id: &str) {
    if !workspace.exists() { return; }
    let root = dir(session_dir);
    let _ = std::fs::create_dir_all(&root);
    let dest = root.join(turn_id);
    if dest.exists() { return; }
    clone_tree(workspace, &dest);
    prune(&root);
}

fn prune(root: &Path) {
    let mut items: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    items.sort();
    let n = items.len();
    for (_, p) in items.into_iter().take(n.saturating_sub(KEEP)) { let _ = std::fs::remove_dir_all(p); }
}

pub fn exists(session_dir: &Path, turn_id: &str) -> bool { dir(session_dir).join(turn_id).exists() }

pub fn restore(session_dir: &Path, workspace: &Path, turn_id: &str) -> Result<(), String> {
    let src = dir(session_dir).join(turn_id);
    if !src.exists() { return Err("snapshot not found".into()); }
    let tmp = workspace.with_file_name(format!("workspace.rewind-{}", crate::text::short_id(4)));
    if !clone_tree(&src, &tmp) { return Err("snapshot copy failed".into()); }
    let _ = std::fs::remove_dir_all(workspace);
    std::fs::rename(&tmp, workspace).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn take_and_restore() {
        let d = std::env::temp_dir().join(format!("dpsnap-{}", crate::text::short_id(6)));
        let ws = d.join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "v1").unwrap();
        take(&d, &ws, "t1");
        std::fs::write(ws.join("a.txt"), "v2").unwrap();
        std::fs::write(ws.join("b.txt"), "new").unwrap();
        assert!(exists(&d, "t1"));
        restore(&d, &ws, "t1").unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "v1");
        assert!(!ws.join("b.txt").exists());
        assert!(restore(&d, &ws, "nope").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
