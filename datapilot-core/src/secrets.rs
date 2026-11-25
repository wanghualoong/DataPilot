//! Platform adapter: secret storage (L3 of ADR-0012). macOS → login keychain via /usr/bin/security
//! (same services as the Swift app); tests / other hosts → static values.
use std::collections::HashMap;

pub trait SecretStore: Send + Sync {
    fn api_key(&self, provider_id: &str) -> Option<String>;
    fn db_password(&self, connection_id: &str) -> Option<String>;
    /// SSH password or private-key passphrase of a database connection's tunnel.
    fn ssh_secret(&self, _connection_id: &str) -> Option<String> { None }
    /// Write (Some) or delete (None) a secret. `kind`: api_key | db_password | ssh_secret. Write-only for UIs.
    fn set(&self, _kind: &str, _id: &str, _value: Option<&str>) -> Result<(), String> { Err("secret storage is read-only here".into()) }
}

pub fn service_for(kind: &str) -> Result<&'static str, String> {
    match kind {
        "api_key" => Ok(Keychain::API_SERVICE),
        "db_password" => Ok(Keychain::DB_SERVICE),
        "ssh_secret" => Ok(Keychain::SSH_SERVICE),
        k => Err(format!("unknown secret kind {k}")),
    }
}

pub struct Keychain;

impl Keychain {
    pub const API_SERVICE: &'static str = "io.github.datapilot.apikeys";
    pub const DB_SERVICE: &'static str = "io.github.datapilot.dbpasswords";
    pub const SSH_SERVICE: &'static str = "io.github.datapilot.sshsecrets";
    fn read(service: &str, account: &str) -> Option<String> {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", account, "-w"]).output().ok()?;
        if !out.status.success() { return None; }
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if v.is_empty() { None } else { Some(v) }
    }

    /// The value goes through stdin of `security -i`, never argv (same as the Swift `KeychainStore.write`).
    pub fn write(service: &str, account: &str, value: &str, label: &str) -> Result<(), String> {
        use std::io::Write;
        if [service, account, label].iter().any(|s| s.contains('\n')) || value.contains('\n') { return Err("invalid characters".into()); }
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let cmd = format!("add-generic-password -U -s {} -a {} -l {} -w {}\n", q(service), q(account), q(label), q(value));
        let mut c = std::process::Command::new("/usr/bin/security").arg("-i")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().map_err(|e| e.to_string())?;
        c.stdin.take().ok_or("no stdin")?.write_all(cmd.as_bytes()).map_err(|e| e.to_string())?;
        let st = c.wait().map_err(|e| e.to_string())?;
        if st.success() { Ok(()) } else { Err("写入钥匙串失败 / keychain write failed".into()) }
    }

    pub fn delete(service: &str, account: &str) {
        let _ = std::process::Command::new("/usr/bin/security").args(["delete-generic-password", "-s", service, "-a", account])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
}

impl SecretStore for Keychain {
    fn api_key(&self, p: &str) -> Option<String> { Self::read(Self::API_SERVICE, p) }
    fn db_password(&self, c: &str) -> Option<String> { Self::read(Self::DB_SERVICE, c) }
    fn ssh_secret(&self, c: &str) -> Option<String> { Self::read(Self::SSH_SERVICE, c) }
    fn set(&self, kind: &str, id: &str, value: Option<&str>) -> Result<(), String> {
        let svc = service_for(kind)?;
        if id.trim().is_empty() { return Err("missing id".into()); }
        Self::delete(svc, id);
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => Self::write(svc, id, v, match kind { "api_key" => "DataPilot API key", "db_password" => "DataPilot database password", _ => "DataPilot SSH secret" }),
            None => Ok(()),
        }
    }
}

/// Windows Credential Manager (generic credentials). Target = "<service>/<id>", same service names as the macOS keychain.
#[cfg(windows)]
pub struct WinCred;

#[cfg(windows)]
impl WinCred {
    fn target(service: &str, account: &str) -> Vec<u16> { format!("{service}/{account}").encode_utf16().chain(std::iter::once(0)).collect() }
    pub fn read(service: &str, account: &str) -> Option<String> {
        use windows_sys::Win32::Security::Credentials::*;
        let t = Self::target(service, account);
        unsafe {
            let mut c: *mut CREDENTIALW = std::ptr::null_mut();
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut c) == 0 || c.is_null() { return None; }
            let blob = std::slice::from_raw_parts((*c).CredentialBlob, (*c).CredentialBlobSize as usize).to_vec();
            CredFree(c as _);
            String::from_utf8(blob).ok().filter(|v| !v.is_empty())
        }
    }
    pub fn write(service: &str, account: &str, value: &str) -> Result<(), String> {
        use windows_sys::Win32::Security::Credentials::*;
        let mut t = Self::target(service, account);
        let mut user: Vec<u16> = account.encode_utf16().chain(std::iter::once(0)).collect();
        let mut blob = value.as_bytes().to_vec();
        let mut c: CREDENTIALW = unsafe { std::mem::zeroed() };
        c.Type = CRED_TYPE_GENERIC;
        c.TargetName = t.as_mut_ptr();
        c.UserName = user.as_mut_ptr();
        c.CredentialBlobSize = blob.len() as u32;
        c.CredentialBlob = blob.as_mut_ptr();
        c.Persist = CRED_PERSIST_LOCAL_MACHINE;
        let ok = unsafe { CredWriteW(&c, 0) } != 0;
        blob.iter_mut().for_each(|b| *b = 0);
        if ok { Ok(()) } else { Err(format!("写入凭据管理器失败 / Credential Manager write failed ({})", std::io::Error::last_os_error())) }
    }
    pub fn delete(service: &str, account: &str) {
        use windows_sys::Win32::Security::Credentials::*;
        let t = Self::target(service, account);
        unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0); }
    }
}

#[cfg(windows)]
impl SecretStore for WinCred {
    fn api_key(&self, p: &str) -> Option<String> { Self::read(Keychain::API_SERVICE, p) }
    fn db_password(&self, c: &str) -> Option<String> { Self::read(Keychain::DB_SERVICE, c) }
    fn ssh_secret(&self, c: &str) -> Option<String> { Self::read(Keychain::SSH_SERVICE, c) }
    fn set(&self, kind: &str, id: &str, value: Option<&str>) -> Result<(), String> {
        let svc = service_for(kind)?;
        if id.trim().is_empty() { return Err("missing id".into()); }
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => Self::write(svc, id, v),
            None => { Self::delete(svc, id); Ok(()) }
        }
    }
}

/// The OS secret store for this platform.
pub fn system_store() -> std::sync::Arc<dyn SecretStore> {
    #[cfg(windows)]
    { std::sync::Arc::new(WinCred) }
    #[cfg(not(windows))]
    { std::sync::Arc::new(Keychain) }
}

#[derive(Default)]
pub struct StaticSecrets { pub api_keys: HashMap<String, String>, pub default_key: Option<String>, pub db: HashMap<String, String>,
                           pub written: std::sync::Mutex<HashMap<(String, String), String>> }

impl SecretStore for StaticSecrets {
    fn api_key(&self, p: &str) -> Option<String> {
        self.written.lock().unwrap().get(&("api_key".into(), p.into())).cloned().or_else(|| self.api_keys.get(p).cloned()).or_else(|| self.default_key.clone())
    }
    fn db_password(&self, c: &str) -> Option<String> { self.written.lock().unwrap().get(&("db_password".into(), c.into())).cloned().or_else(|| self.db.get(c).cloned()) }
    fn ssh_secret(&self, c: &str) -> Option<String> { self.written.lock().unwrap().get(&("ssh_secret".into(), c.into())).cloned() }
    fn set(&self, kind: &str, id: &str, value: Option<&str>) -> Result<(), String> {
        service_for(kind)?;
        let k = (kind.to_string(), id.to_string());
        let mut w = self.written.lock().unwrap();
        match value.filter(|v| !v.is_empty()) { Some(v) => { w.insert(k, v.to_string()); } None => { w.remove(&k); } }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Real keychain round trip under a throwaway service name (removed afterwards); skipped off macOS.
    #[test]
    fn keychain_write_read_delete() {
        if !std::path::Path::new("/usr/bin/security").exists() { return; }
        let svc = format!("io.github.datapilot.test-{}", crate::text::short_id(6));
        let v = "p@ss \"q\" \\ w0rd";
        if Keychain::write(&svc, "acct", v, "DataPilot test").is_err() { return; }   // locked keychain in CI
        assert_eq!(Keychain::read(&svc, "acct").as_deref(), Some(v));
        Keychain::delete(&svc, "acct");
        assert!(Keychain::read(&svc, "acct").is_none());
    }
}
