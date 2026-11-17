//! OS sandbox for the Python kernel and the bash tool (ADR-0012 L3).
//! macOS: Seatbelt (`sandbox-exec`, Resources/sandbox/*.sbpl). Windows: AppContainer `DataPilot.Kernel` + Job Object.
//! Both: write access only to workspace / tmp / outputs, read access to the project, Python and Resources,
//! network only when the user's switch is on (or, for the kernel, database ports — on Windows that is the coarser
//! internetClient + privateNetworkClientServer capability pair).
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::paths::Paths;

#[cfg(not(windows))] mod seatbelt;
#[cfg(not(windows))] use seatbelt as imp;
#[cfg(not(windows))] pub use seatbelt::db_port_rules;
#[cfg(windows)] mod appcontainer;
#[cfg(windows)] use appcontainer as imp;
#[cfg(windows)] pub use appcontainer::{loopback_exempt, quote_arg, CONTAINER};

pub struct Policy {
    pub workspace: PathBuf,
    pub tmp: PathBuf,
    pub paths: Paths,
    pub network: bool,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub db_ports: Vec<u16>,
}

impl Policy {
    /// Base environment for sandboxed children (the parent environment is never inherited).
    pub fn env(&self) -> Vec<(String, String)> { imp::env(self) }
}

/// A sandboxed child. `kill` ends the whole process tree on Windows (Job Object).
pub struct Proc(imp::Proc);

impl Proc {
    /// None while running; exit code (-1 when unknown) once finished.
    pub fn try_wait(&mut self) -> Option<i32> { self.0.try_wait() }
    pub fn kill(&mut self) { self.0.kill() }
    pub fn wait(&mut self) -> i32 { self.0.wait() }
}

pub struct Spawned {
    pub proc: Proc,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Box<dyn Read + Send>,
    pub stderr: Box<dyn Read + Send>,
}

/// Start `cmd` (absolute program path first) inside the sandbox with exactly `env` as its environment.
pub fn spawn(policy: &Policy, cmd: &[String], env: &[(String, String)], cwd: &Path, stdin_piped: bool) -> Result<Spawned, String> {
    if cmd.is_empty() { return Err("empty command".into()); }
    let (proc, stdin, stdout, stderr) = imp::spawn(policy, cmd, env, cwd, stdin_piped)?;
    Ok(Spawned { proc: Proc(proc), stdin, stdout, stderr })
}

/// argv that runs a shell command line (macOS: zsh -c; Windows: PowerShell -EncodedCommand).
pub fn shell(command: &str) -> Vec<String> { imp::shell(command) }

/// Does this stderr look like a sandbox denial?
pub fn denied(stderr: &str) -> bool {
    ["Operation not permitted", "Access is denied", "拒绝访问", "PermissionError", "UnauthorizedAccess"].iter().any(|k| stderr.contains(k))
}

/// Does this stderr look like "no network"?
pub fn offline(stderr: &str) -> bool {
    ["Could not resolve", "nodename nor servname", "getaddrinfo failed", "No such host is known", "不知道这样的主机", "Name or service not known"]
        .iter().any(|k| stderr.contains(k))
}
