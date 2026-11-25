//! SSH tunnels for database connections: `/usr/bin/ssh -N -L 127.0.0.1:<local>:<db_host>:<db_port>`, run by the core
//! (outside the kernel sandbox). The kernel then connects to 127.0.0.1:<local>; that local port is the one its
//! sandbox allows (`kernel::db_ports`). The local port is derived from the connection id, so it is stable across
//! restarts and known before the kernel starts. Passwords / key passphrases go to ssh through SSH_ASKPASS from an
//! environment variable — never through argv or a file.
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};


#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub key: Option<PathBuf>,
    pub db_host: String,
    pub db_port: u16,
    pub local_port: u16,
    /// Tests only: a known_hosts file instead of the user's ~/.ssh/known_hosts.
    pub known_hosts: Option<PathBuf>,
}

static TUNNELS: Mutex<Option<HashMap<String, (Child, Spec)>>> = Mutex::new(None);

pub fn enabled(conn: &Value) -> bool {
    conn["ssh"] == true && conn["ssh_host"].as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Stable local port for a connection id (40000–59999).
pub fn local_port(id: &str) -> u16 {
    let mut h: u32 = 0x811c9dc5;
    for b in id.bytes() { h ^= b as u32; h = h.wrapping_mul(0x01000193); }
    40000 + (h % 20000) as u16
}

fn default_db_port(t: &str) -> Option<u16> {
    match t { "postgres" => Some(5432), "mysql" => Some(3306), "sqlserver" => Some(1433), "oracle" => Some(1521), "clickhouse" => Some(8123), _ => None }
}

/// Host / user names go into ssh's argv: no leading '-', no whitespace or shell-ish characters.
fn safe(s: &str, what: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('-') || s.chars().any(|c| c.is_whitespace() || "'\"`$;|&<>\\".contains(c)) {
        return Err(format!("SSH 设置无效：{what} = {s:?}"));
    }
    Ok(s.to_string())
}

fn port(v: &Value) -> Option<u16> { v.as_u64().map(|p| p as u16).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|p| *p > 0) }

pub fn spec(id: &str, conn: &Value, home: &Path) -> Result<Spec, String> {
    let t = conn["type"].as_str().unwrap_or("");
    let db_port = port(&conn["port"]).or_else(|| default_db_port(t)).ok_or("数据库端口未知 / unknown database port")?;
    let key = conn["ssh_key"].as_str().map(str::trim).filter(|k| !k.is_empty()).map(|k| {
        if let Some(rest) = k.strip_prefix("~/") { home.join(rest) } else { PathBuf::from(k) }
    });
    if let Some(k) = &key { if !k.is_file() { return Err(format!("找不到 SSH 私钥文件：{}", k.display())); } }
    let user = conn["ssh_user"].as_str().map(str::trim).filter(|u| !u.is_empty()).map(String::from)
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_default());
    Ok(Spec { ssh_host: safe(conn["ssh_host"].as_str().unwrap_or(""), "SSH 主机")?, ssh_port: port(&conn["ssh_port"]).unwrap_or(22),
              ssh_user: safe(&user, "SSH 用户")?, key, db_host: safe(conn["host"].as_str().unwrap_or("localhost"), "数据库主机")?,
              db_port, local_port: local_port(id), known_hosts: None })
}

fn port_open(p: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], p)), Duration::from_millis(300)).is_ok()
}

/// Prints the secret from the environment (ssh calls it with the prompt as argv[1]); the secret itself is never on disk.
fn askpass_script(app_support: &Path) -> Result<PathBuf, String> {
    #[cfg(not(unix))]
    let (p, body) = (app_support.join("ssh-askpass.cmd"),
                     "@powershell -NoProfile -NonInteractive -Command \"[Console]::Out.WriteLine($env:DP_SSH_SECRET)\"\r\n");
    #[cfg(unix)]
    let p = app_support.join("ssh-askpass.sh");
    #[cfg(unix)]
    let body = "#!/bin/sh\nprintf '%s\\n' \"$DP_SSH_SECRET\"\n";
    if std::fs::read_to_string(&p).ok().as_deref() != Some(body) {
        std::fs::create_dir_all(app_support).map_err(|e| e.to_string())?;
        std::fs::write(&p, body).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(p)
}

fn spawn(sp: &Spec, secret: Option<&str>, app_support: &Path) -> Result<Child, String> {
    let mut c = Command::new(crate::platform::ssh_exe());   // fixed path, never resolved through PATH
    c.args(["-N", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=3",
            "-o", "ConnectTimeout=15", "-o", "StrictHostKeyChecking=accept-new", "-o", "NumberOfPasswordPrompts=1"]);
    if let Some(k) = &sp.known_hosts { c.arg("-o").arg(format!("UserKnownHostsFile={}", k.display())); }
    c.arg("-p").arg(sp.ssh_port.to_string());
    if let Some(k) = &sp.key { c.arg("-i").arg(k).args(["-o", "IdentitiesOnly=yes"]); }
    c.arg("-L").arg(format!("127.0.0.1:{}:{}:{}", sp.local_port, sp.db_host, sp.db_port));
    c.arg(format!("{}@{}", sp.ssh_user, sp.ssh_host));
    match secret.filter(|s| !s.is_empty()) {
        Some(s) => {
            c.env("SSH_ASKPASS", askpass_script(app_support)?).env("SSH_ASKPASS_REQUIRE", "force").env("DP_SSH_SECRET", s).env_remove("DISPLAY");
        }
        None => { c.args(["-o", "BatchMode=yes"]); }
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); }   // CREATE_NO_WINDOW: no console pop-up
    c.spawn().map_err(|e| format!("无法启动 ssh：{e}"))
}

fn kill(mut c: Child) { let _ = c.kill(); let _ = c.wait(); }

/// Start (or reuse) the tunnel for connection `id`; returns the local port to connect to.
pub fn ensure(id: &str, conn: &Value, secret: Option<&str>, app_support: &Path) -> Result<u16, String> {
    let home = crate::platform::home();
    ensure_spec(id, spec(id, conn, &home)?, secret, app_support)
}

pub fn ensure_spec(id: &str, sp: Spec, secret: Option<&str>, app_support: &Path) -> Result<u16, String> {
    let mut g = TUNNELS.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if let Some((child, old)) = m.get_mut(id) {
        if *old == sp && matches!(child.try_wait(), Ok(None)) && port_open(sp.local_port) { return Ok(sp.local_port); }
    }
    if let Some((c, _)) = m.remove(id) { kill(c); }
    if port_open(sp.local_port) {
        return Err(format!("本机端口 {} 已被其他程序占用，无法建立 SSH 隧道", sp.local_port));
    }
    let mut child = spawn(&sp, secret, app_support)?;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(25) {
        if port_open(sp.local_port) {
            m.insert(id.to_string(), (child, sp.clone()));
            crate::log::info("ssh_tunnel", "", &[("id", id.to_string()), ("ok", "1".into()), ("ms", t0.elapsed().as_millis().to_string())]);
            return Ok(sp.local_port);
        }
        if let Ok(Some(_)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() { let _ = e.read_to_string(&mut err); }
            let err: String = err.lines().filter(|l| !l.trim().is_empty() && !l.contains("Permanently added")).collect::<Vec<_>>().join("；");
            crate::log::warn("ssh_tunnel", "", &[("id", id.to_string()), ("ok", "0".into()), ("ms", t0.elapsed().as_millis().to_string()), ("err", crate::log::err_text(&err))]);
            return Err(format!("SSH 隧道建立失败：{}", if err.is_empty() { "ssh 已退出".into() } else { crate::text::prefix(&err, 400) }));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    kill(child);
    crate::log::warn("ssh_tunnel", "", &[("id", id.to_string()), ("ok", "0".into()), ("err", "timeout 25s".into())]);
    Err("SSH 隧道建立超时（25 秒）：检查 SSH 主机、端口和网络".into())
}

/// Kernel ops that open a database connection.
pub fn opens_connection(op: &str) -> bool {
    matches!(op, "db_test" | "db_schema" | "db_query" | "db_extract" | "db_refresh" | "explain")
}

/// If `conn` uses an SSH tunnel: start / reuse it and point the kernel at it (`host_override` / `port_override`).
/// The tunnel's local port must already be allowed by the kernel sandbox, i.e. the connection is saved with SSH on.
pub fn inject(conn: &Value, workspace: &Path, params: &mut serde_json::Map<String, Value>, secret: Option<String>, app_support: &Path) -> Result<(), String> {
    if !enabled(conn) { return Ok(()); }
    let id = conn["id"].as_str().unwrap_or("");
    if id.is_empty() || !crate::kernel::db_ports(workspace).contains(&local_port(id)) {
        return Err("开启 SSH 隧道后请先保存连接，再在列表里测试 / Save the connection with SSH enabled first, then test it from the list".into());
    }
    let p = ensure(id, conn, secret.as_deref(), app_support)?;
    params.insert("host_override".into(), Value::from("127.0.0.1"));
    params.insert("port_override".into(), Value::from(p));
    Ok(())
}

pub fn close(id: &str) {
    if let Some((c, _)) = TUNNELS.lock().unwrap().as_mut().and_then(|m| m.remove(id)) { kill(c); }
}

pub fn shutdown_all() {
    if let Some(m) = TUNNELS.lock().unwrap().take() { for (_, (c, _)) in m { kill(c); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_and_validation() {
        assert_eq!(local_port("db_abc"), local_port("db_abc"));
        assert!((40000..60000).contains(&local_port("x")));
        let home = std::env::temp_dir();
        let s = spec("db_1", &json!({"type": "postgres", "host": "10.0.0.5", "ssh": true, "ssh_host": "jump", "ssh_user": "u"}), &home).unwrap();
        assert_eq!((s.db_port, s.ssh_port, s.ssh_host.as_str(), s.db_host.as_str()), (5432, 22, "jump", "10.0.0.5"));
        assert!(spec("d", &json!({"type": "mysql", "ssh_host": "-oProxyCommand=x", "ssh": true}), &home).is_err());
        assert!(spec("d", &json!({"type": "mysql", "ssh_host": "h", "ssh_user": "a b"}), &home).is_err());
        assert!(spec("d", &json!({"type": "mysql", "ssh_host": "h", "ssh_key": "~/no-such-key-dp"}), &home).is_err());
        assert!(enabled(&json!({"ssh": true, "ssh_host": "h"})) && !enabled(&json!({"ssh": false, "ssh_host": "h"})));
    }

    /// Real sshd on 127.0.0.1 (throwaway host / client keys, pubkey auth as the current user) forwarding to a local
    /// TCP echo server: key without passphrase (BatchMode) and key with passphrase (SSH_ASKPASS). Skipped when sshd
    /// cannot start (e.g. CI without OpenSSH server).
    #[test]
    fn live_tunnel() {
        use std::io::Write;
        let d = std::env::temp_dir().join(format!("dpssh-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&d).unwrap();
        let kg = |name: &str, pass: &str| Command::new("/usr/bin/ssh-keygen").args(["-q", "-t", "ed25519", "-N", pass, "-f"]).arg(d.join(name)).status().map(|s| s.success()).unwrap_or(false);
        if !(kg("hk", "") && kg("ck", "") && kg("ckp", "pp-123")) { return; }
        let pubs = format!("{}{}", std::fs::read_to_string(d.join("ck.pub")).unwrap(), std::fs::read_to_string(d.join("ckp.pub")).unwrap());
        std::fs::write(d.join("authorized_keys"), pubs).unwrap();
        let sshd_port = 23000 + (std::process::id() % 1000) as u16;
        let cfg = format!("Port {sshd_port}\nListenAddress 127.0.0.1\nHostKey {0}/hk\nAuthorizedKeysFile {0}/authorized_keys\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPidFile {0}/sshd.pid\nAllowTcpForwarding yes\n", d.display());
        std::fs::write(d.join("sshd_config"), cfg).unwrap();
        let Ok(mut sshd) = Command::new("/usr/sbin/sshd").args(["-D", "-f"]).arg(d.join("sshd_config")).stderr(Stdio::null()).spawn() else { return };
        std::thread::sleep(Duration::from_millis(500));
        if !port_open(sshd_port) { kill(sshd); return; }
        // echo server = the "database"
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let echo = l.local_addr().unwrap().port();
        std::thread::spawn(move || for s in l.incoming().flatten() {
            std::thread::spawn(move || { let mut s = s; let mut b = [0u8; 64]; if let Ok(n) = s.read(&mut b) { let _ = s.write_all(&b[..n]); } });
        });
        let user = std::env::var("USER").unwrap();
        let mk = |key: &str, lp: u16| Spec { ssh_host: "127.0.0.1".into(), ssh_port: sshd_port, ssh_user: user.clone(), key: Some(d.join(key)),
                                              db_host: "127.0.0.1".into(), db_port: echo, local_port: lp, known_hosts: Some(d.join("known_hosts")) };
        let roundtrip = |p: u16| { let mut s = TcpStream::connect(("127.0.0.1", p)).unwrap(); s.write_all(b"ping").unwrap(); let mut b = [0u8; 4]; s.read_exact(&mut b).unwrap(); b };
        let p1 = ensure_spec("t-plain", mk("ck", 0).clone_with_port(local_port("t-plain")), None, &d).unwrap();
        assert_eq!(&roundtrip(p1), b"ping");
        assert_eq!(ensure_spec("t-plain", mk("ck", local_port("t-plain")), None, &d).unwrap(), p1); // reused
        let p2 = ensure_spec("t-pass", mk("ckp", local_port("t-pass")), Some("pp-123"), &d).unwrap();
        assert_eq!(&roundtrip(p2), b"ping");
        let e = ensure_spec("t-bad", mk("ckp", local_port("t-bad")), Some("wrong"), &d).unwrap_err();
        assert!(e.contains("SSH 隧道建立失败"), "{e}");
        close("t-plain"); close("t-pass");
        std::thread::sleep(Duration::from_millis(200));
        assert!(!port_open(p1));
        kill(sshd);
        let _ = std::fs::remove_dir_all(&d);
    }

    impl Spec { fn clone_with_port(mut self, p: u16) -> Spec { self.local_port = p; self } }
}
