//! macOS Seatbelt backend (policy adapted from openai/codex, Apache-2.0 — Resources/sandbox/*.sbpl).
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use super::Policy;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec"; // never resolved through PATH

fn real(p: &Path) -> PathBuf { crate::platform::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) }

fn text(p: &Policy) -> Result<String, String> {
    let base = p.paths.resources.join("sandbox/base.sbpl");
    let mut t = std::fs::read_to_string(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    for i in 0..p.read_roots.len() { t += &format!("\n(allow file-read* (subpath (param \"READ_ROOT_{i}\")))"); }
    for i in 0..p.write_roots.len() { t += &format!("\n(allow file-read* file-write* (subpath (param \"WRITE_ROOT_{i}\")))"); }
    if p.network {
        t += "\n";
        t += &std::fs::read_to_string(p.paths.resources.join("sandbox/network.sbpl")).map_err(|e| e.to_string())?;
    } else if !p.db_ports.is_empty() {
        t += "\n";
        t += &db_port_rules(&p.db_ports);
    }
    Ok(t)
}

fn args(p: &Policy, command: &[String]) -> Result<Vec<String>, String> {
    let mut a = vec!["-p".to_string(), text(p)?,
                     format!("-DWORKSPACE={}", real(&p.workspace).display()), format!("-DTMPDIR={}", real(&p.tmp).display()),
                     format!("-DPYROOT={}", real(&p.paths.python_root).display()), format!("-DRESROOT={}", real(&p.paths.resources).display())];
    for (i, r) in p.read_roots.iter().enumerate() { a.push(format!("-DREAD_ROOT_{i}={}", real(r).display())); }
    for (i, r) in p.write_roots.iter().enumerate() { a.push(format!("-DWRITE_ROOT_{i}={}", real(r).display())); }
    a.push("--".into());
    a.extend(command.iter().cloned());
    Ok(a)
}

pub fn env(p: &Policy) -> Vec<(String, String)> {
    let ws = real(&p.workspace).display().to_string();
    let tmp = real(&p.tmp).display().to_string();
    vec![("HOME".into(), ws.clone()), ("TMPDIR".into(), tmp.clone()), ("DP_WORKSPACE".into(), ws),
         ("PATH".into(), format!("{}/bin:/usr/bin:/bin:/usr/sbin:/sbin", p.paths.python_root.display())),
         ("MPLCONFIGDIR".into(), tmp), ("MPLBACKEND".into(), "Agg".into()), ("PYTHONIOENCODING".into(), "utf-8".into()),
         ("LANG".into(), "en_US.UTF-8".into()), ("PYTHONDONTWRITEBYTECODE".into(), "1".into())]
}

/// Only the listed database ports (+ DNS) — same rules as Swift `SandboxPolicy.dbPortRules`.
pub fn db_port_rules(ports: &[u16]) -> String {
    let mut ps: Vec<u16> = ports.iter().copied().filter(|p| *p > 0).collect();
    ps.sort(); ps.dedup();
    if ps.is_empty() { return String::new(); }
    let mut t = String::from("; Database connections: only these TCP ports (network switch is off).\n(allow system-socket)\n");
    t += &ps.iter().map(|p| format!("(allow network-outbound (remote tcp \"*:{p}\"))")).collect::<Vec<_>>().join("\n");
    t += "\n(allow network-outbound (literal \"/private/var/run/mDNSResponder\"))";
    t += "\n(allow mach-lookup (global-name \"com.apple.dnssd.service\") (global-name \"com.apple.mDNSResponder\") (global-name \"com.apple.SystemConfiguration.configd\") (global-name \"com.apple.networkd\") (global-name \"com.apple.SecurityServer\") (global-name \"com.apple.trustd.agent\"))";
    t += "\n(allow file-read* (subpath \"/private/var/run\") (literal \"/private/var/run/resolv.conf\"))\n";
    t
}

pub fn shell(command: &str) -> Vec<String> { vec!["/bin/zsh".into(), "-c".into(), command.into()] }

pub struct Proc(Child);

impl Proc {
    pub fn try_wait(&mut self) -> Option<i32> {
        match self.0.try_wait() { Ok(Some(s)) => Some(s.code().unwrap_or(-1)), Ok(None) => None, Err(_) => Some(-1) }
    }
    pub fn kill(&mut self) { let _ = self.0.kill(); }
    pub fn wait(&mut self) -> i32 { self.0.wait().ok().and_then(|s| s.code()).unwrap_or(-1) }
}

type Pipes = (Proc, Option<Box<dyn Write + Send>>, Box<dyn Read + Send>, Box<dyn Read + Send>);

pub fn spawn(p: &Policy, cmd: &[String], env: &[(String, String)], cwd: &Path, stdin_piped: bool) -> Result<Pipes, String> {
    let mut c = Command::new(SANDBOX_EXEC);
    c.args(args(p, cmd)?).env_clear().envs(env.iter().map(|(k, v)| (k, v))).current_dir(cwd)
        .stdin(if stdin_piped { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("cannot start sandboxed process: {e}"))?;
    let stdin = child.stdin.take().map(|s| Box::new(s) as Box<dyn Write + Send>);
    let stdout = Box::new(child.stdout.take().ok_or("no stdout")?);
    let stderr = Box::new(child.stderr.take().ok_or("no stderr")?);
    Ok((Proc(child), stdin, stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn port_rules() {
        let t = db_port_rules(&[5432, 3306, 5432, 0]);
        assert!(t.contains("(remote tcp \"*:5432\")") && t.contains("(remote tcp \"*:3306\")"));
        assert!(!t.contains("*:0\""));
        assert!(db_port_rules(&[]).is_empty());
    }
}
