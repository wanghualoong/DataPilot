//! Windows backend: AppContainer `DataPilot.Kernel` + Job Object (KILL_ON_JOB_CLOSE).
//! - File access: the container SID gets ACL entries — workspace / tmp / outputs full access, project / Python / Resources
//!   read + execute (best effort: folders under Program Files are usually readable by all app packages already).
//! - Network: none by default; when the network switch is on, or the workspace has database connections, the
//!   internetClient + privateNetworkClientServer capabilities (coarser than the macOS port list — documented).
//!   AppContainers cannot reach 127.0.0.1 (SSH tunnels) unless the installer ran
//!   `CheckNetIsolation LoopbackExempt -a -n=DataPilot.Kernel`.
//! - Lifetime: every sandboxed process tree is in its own Job; killing / dropping it ends the whole tree.
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::Isolation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW, FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::*;

use super::Policy;

pub const CONTAINER: &str = "DataPilot.Kernel";
const CAP_INTERNET_CLIENT: &str = "S-1-15-3-1";
const CAP_PRIVATE_NETWORK: &str = "S-1-15-3-3";

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn wide_path(p: &Path) -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() }
fn last_err() -> String { std::io::Error::last_os_error().to_string() }

/// SID owned by us: FreeSid (AppContainer APIs) or LocalFree (ConvertStringSidToSid).
struct Sid { p: PSID, local: bool }
impl Drop for Sid {
    fn drop(&mut self) { unsafe { if self.local { LocalFree(self.p as _); } else { FreeSid(self.p); } } }
}

fn container_sid() -> Result<Sid, String> {
    let name = wide(CONTAINER);
    let display = wide("DataPilot kernel sandbox");
    let mut sid: PSID = null_mut();
    unsafe {
        let hr = CreateAppContainerProfile(name.as_ptr(), display.as_ptr(), display.as_ptr(), null(), 0, &mut sid);
        if hr >= 0 && !sid.is_null() { return Ok(Sid { p: sid, local: false }); }
        // usually HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS): the profile exists from an earlier run
        let hr2 = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
        if hr2 >= 0 && !sid.is_null() { Ok(Sid { p: sid, local: false }) } else { Err(format!("无法创建 AppContainer 沙箱（0x{:08x} / 0x{:08x}）", hr as u32, hr2 as u32)) }
    }
}

fn capability(s: &str) -> Result<Sid, String> {
    let w = wide(s);
    let mut sid: PSID = null_mut();
    if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut sid) } == 0 { return Err(format!("capability {s}: {}", last_err())); }
    Ok(Sid { p: sid, local: true })
}

/// read-only grants that failed (usually Program Files): log once. Write grants are never cached — the folder may have
/// been replaced since (backup restore moves a whole .datapilot/ in, keeping the ACL of where it came from).
static GRANTED: Mutex<Option<HashSet<(PathBuf, bool)>>> = Mutex::new(None);
/// workspaces whose kernel could not use its own folders inside the AppContainer even after `repair`: run unisolated
static RELAXED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn norm(p: &Path) -> PathBuf { PathBuf::from(p.to_string_lossy().replace('/', "\\")) }
/// volumes already reported as unusable for the AppContainer (log once)
static NO_ACL: Mutex<Option<HashSet<String>>> = Mutex::new(None);

const DRIVE_REMOTE: u32 = 4;
const FILE_PERSISTENT_ACLS: u32 = 0x8;

/// Why an AppContainer cannot use `path` at all, if so: its volume keeps no ACLs (exFAT / FAT32 USB drives, some
/// virtual drives — lowbox tokens are denied on them whatever we grant) or is a network share (needs credentials the
/// container does not have). The kernel then failed with "PermissionError: Access is denied: 'E:\\'".
pub fn volume_problem(path: &Path) -> Option<String> {
    let w = wide_path(path);
    let mut root = [0u16; 520];
    unsafe {
        if GetVolumePathNameW(w.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 { return None; }
        let n = root.iter().position(|&c| c == 0).unwrap_or(0);
        let rs = String::from_utf16_lossy(&root[..n]);
        // a whole drive as project (E:\\): an inheritable grant there would rewrite the ACLs of every file on the drive
        if path.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(rs.trim_end_matches(['\\', '/'])) {
            return Some(format!("{rs} 是整个磁盘的根目录"));
        }
        if GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE { return Some(format!("{rs} 是网络驱动器")); }
        let (mut flags, mut fs) = (0u32, [0u16; 64]);
        if GetVolumeInformationW(root.as_ptr(), null_mut(), 0, null_mut(), null_mut(), &mut flags, fs.as_mut_ptr(), fs.len() as u32) == 0 { return None; }
        if flags & FILE_PERSISTENT_ACLS == 0 {
            let k = fs.iter().position(|&c| c == 0).unwrap_or(0);
            return Some(format!("{rs} 是 {} 格式，不支持文件权限", String::from_utf16_lossy(&fs[..k])));
        }
    }
    None
}

/// Does the DACL already allow `sid` the rights in `need` (an explicit allow ACE)?
unsafe fn has_ace(dacl: *const ACL, sid: PSID, need: u32) -> bool {
    if dacl.is_null() { return false; }
    for i in 0..(*dacl).AceCount as u32 {
        let mut ace: *mut core::ffi::c_void = null_mut();
        if GetAce(dacl, i, &mut ace) == 0 || ace.is_null() { continue; }
        let a = ace as *const ACCESS_ALLOWED_ACE;
        if (*a).Header.AceType != 0 { continue; }   // ACCESS_ALLOWED_ACE_TYPE
        if (*a).Header.AceFlags & 0x08 != 0 { continue; }   // INHERIT_ONLY_ACE: does not apply to the folder itself
        let ace_sid = std::ptr::addr_of!((*a).SidStart) as PSID;
        if EqualSid(ace_sid, sid) != 0 && (*a).Mask & need == need { return true; }
    }
    false
}

/// Add an inheritable allow ACE for the container on `path` (skipped when already present). `tree`: also reset every
/// descendant to inherit it (protected / foreign ACLs below are replaced; explicit ACEs there are kept) — only used on
/// DataPilot's own folders (workspace, tmp, outputs).
fn grant(sid: PSID, path: &Path, write: bool, tree: bool) -> Result<(), String> {
    let path = &norm(path);
    let need = if write { FILE_ALL_ACCESS } else { FILE_GENERIC_READ | FILE_GENERIC_EXECUTE };
    let w = wide_path(path);
    unsafe {
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let r = GetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut dacl, null_mut(), &mut sd);
        if r != 0 { return Err(format!("{}: 读取权限失败（{r}）", path.display())); }
        if tree || !has_ace(dacl, sid, need) {
            let ea = EXPLICIT_ACCESS_W {
                grfAccessPermissions: need, grfAccessMode: GRANT_ACCESS, grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: TRUSTEE_W { pMultipleTrustee: null_mut(), MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE, TrusteeForm: TRUSTEE_IS_SID,
                                     TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP, ptstrName: sid as *mut u16 },
            };
            let mut new_acl: *mut ACL = null_mut();
            let r = SetEntriesInAclW(1, &ea, dacl, &mut new_acl);
            if r != 0 { LocalFree(sd as _); return Err(format!("{}: 设置权限失败（{r}）", path.display())); }
            let r = if tree {
                TreeSetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION, null_mut(), null_mut(),
                                          new_acl, null(), TREE_SEC_INFO_RESET_KEEP_EXPLICIT, None, ProgressInvokeNever, null())
            } else { SetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), new_acl, null()) };
            LocalFree(new_acl as _);
            if r != 0 { LocalFree(sd as _); return Err(format!("{}: 写入权限失败（{r}）", path.display())); }
        }
        LocalFree(sd as _);
    }
    Ok(())
}

/// The sandboxed kernel was denied its own folders: give the container access to the whole workspace / tmp / outputs
/// trees again, replacing whatever ACLs ended up below (moved-in folders, protected ACLs).
pub fn repair(p: &Policy) -> Result<(), String> {
    let sid = container_sid()?;
    for d in [&p.workspace, &p.tmp].into_iter().chain(p.write_roots.iter()) { grant(sid.p, d, true, true)?; }
    Ok(())
}

/// Run this workspace without the AppContainer from now on (until DataPilot restarts). False when it already was.
pub fn relax(p: &Policy, why: &str) -> bool {
    let new = RELAXED.lock().unwrap().get_or_insert_with(HashSet::new).insert(norm(&p.workspace));
    if new {
        eprintln!("sandbox: 沙箱内无法访问工作区，改为不隔离运行（{}）：{why}", p.workspace.display());
        crate::log::warn("sandbox_off", "", &[("why", format!("denied: {why}")), ("ws", p.workspace.display().to_string())]);
    }
    new
}

fn sysroot() -> String { std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()) }

/// Installer helper (runs elevated): create the container profile and exempt it from the loopback block, so the kernel
/// can reach SSH tunnels on 127.0.0.1. `remove` undoes the exemption (uninstall).
pub fn loopback_exempt(remove: bool) -> Result<String, String> {
    let sid = container_sid()?;
    let mut w: *mut u16 = null_mut();
    if unsafe { ConvertSidToStringSidW(sid.p, &mut w) } == 0 { return Err(last_err()); }
    let sid_s = unsafe { let mut n = 0; while *w.add(n) != 0 { n += 1; } let s = String::from_utf16_lossy(std::slice::from_raw_parts(w, n)); LocalFree(w as _); s };
    let exe = format!("{}\\System32\\CheckNetIsolation.exe", sysroot());
    let st = std::process::Command::new(exe).args(["LoopbackExempt", if remove { "-d" } else { "-a" }, &format!("-p={sid_s}")])
        .status().map_err(|e| e.to_string())?;
    if st.success() { Ok(sid_s) } else { Err(format!("CheckNetIsolation failed ({st})")) }
}

pub fn env(p: &Policy) -> Vec<(String, String)> {
    let ws = p.workspace.display().to_string();
    let tmp = p.tmp.display().to_string();
    let py = p.paths.python_root.display().to_string();
    let sys = sysroot();
    let mut v: Vec<(String, String)> = vec![
        ("HOME".into(), ws.clone()), ("USERPROFILE".into(), ws.clone()), ("DP_WORKSPACE".into(), ws),
        ("TEMP".into(), tmp.clone()), ("TMP".into(), tmp.clone()), ("LOCALAPPDATA".into(), tmp.clone()), ("APPDATA".into(), tmp.clone()),
        ("PATH".into(), format!("{py};{py}\\Scripts;{sys}\\System32;{sys};{sys}\\System32\\WindowsPowerShell\\v1.0")),
        ("SystemRoot".into(), sys.clone()), ("windir".into(), sys.clone()), ("ComSpec".into(), format!("{sys}\\System32\\cmd.exe")),
        ("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into()),
        ("MPLCONFIGDIR".into(), tmp), ("MPLBACKEND".into(), "Agg".into()), ("PYTHONIOENCODING".into(), "utf-8".into()),
        ("PYTHONUTF8".into(), "1".into()), ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
    ];
    for k in ["NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS"] {
        if let Ok(x) = std::env::var(k) { v.push((k.into(), x)); }
    }
    v
}

/// PowerShell with the command passed base64-encoded (UTF-16LE): no quoting pitfalls, UTF-8 output.
pub fn shell(command: &str) -> Vec<String> {
    use base64::Engine as _;
    let script = format!("$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}");
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    vec![format!("{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", sysroot()), "-NoProfile".into(), "-NonInteractive".into(),
         "-ExecutionPolicy".into(), "Bypass".into(), "-EncodedCommand".into(), base64::engine::general_purpose::STANDARD.encode(bytes)]
}

/// One argument per the MSVC CommandLineToArgvW rules.
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\u{b}', '"']) { return a.to_string(); }
    let mut s = String::from("\"");
    let mut bs = 0usize;
    for ch in a.chars() {
        match ch {
            '\\' => { bs += 1; continue; }
            '"' => { s.extend(std::iter::repeat('\\').take(bs * 2 + 1)); s.push('"'); }
            c => { s.extend(std::iter::repeat('\\').take(bs)); s.push(c); }
        }
        bs = 0;
    }
    s.extend(std::iter::repeat('\\').take(bs * 2));
    s.push('"');
    s
}

fn env_block(env: &[(String, String)]) -> Vec<u16> {
    let mut e: Vec<&(String, String)> = env.iter().collect();
    e.sort_by_key(|(k, _)| k.to_uppercase());
    e.dedup_by_key(|(k, _)| k.to_uppercase());
    let mut b: Vec<u16> = Vec::new();
    for (k, v) in e { b.extend(format!("{k}={v}").encode_utf16()); b.push(0); }
    if b.is_empty() { b.push(0); }
    b.push(0);
    b
}

pub struct Proc { process: HANDLE, job: HANDLE, code: Option<i32> }
unsafe impl Send for Proc {}

impl Proc {
    fn exit_code(&mut self) -> i32 {
        let mut c: u32 = 0;
        let code = if unsafe { GetExitCodeProcess(self.process, &mut c) } != 0 { c as i32 } else { -1 };
        self.code = Some(code);
        code
    }
    pub fn try_wait(&mut self) -> Option<i32> {
        if let Some(c) = self.code { return Some(c); }
        if unsafe { WaitForSingleObject(self.process, 0) } == WAIT_OBJECT_0 { Some(self.exit_code()) } else { None }
    }
    pub fn kill(&mut self) { unsafe { TerminateJobObject(self.job, 1); } }
    pub fn wait(&mut self) -> i32 {
        if let Some(c) = self.code { return c; }
        unsafe { WaitForSingleObject(self.process, INFINITE); }
        self.exit_code()
    }
}

impl Drop for Proc {
    fn drop(&mut self) { unsafe { CloseHandle(self.process); CloseHandle(self.job); } }   // closing the job ends the tree
}

type Pipes = (Proc, Option<Box<dyn Write + Send>>, Box<dyn Read + Send>, Box<dyn Read + Send>);

/// Anonymous pipe; the end at index `parent` (0 = read, 1 = write) is made non-inheritable.
unsafe fn pipe(parent: usize) -> Result<[HANDLE; 2], String> {
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
    if CreatePipe(&mut r, &mut w, &sa, 0) == 0 { return Err(format!("CreatePipe: {}", last_err())); }
    let h = [r, w];
    SetHandleInformation(h[parent], HANDLE_FLAG_INHERIT, 0);
    Ok(h)
}

pub fn spawn(p: &Policy, cmd: &[String], env: &[(String, String)], cwd: &Path, stdin_piped: bool) -> Result<Pipes, String> {
    let sid = container_sid()?;
    for d in [&p.workspace, &p.tmp].into_iter().chain(p.write_roots.iter()) { let _ = std::fs::create_dir_all(d); }
    // project / workspace on a volume the container cannot use: run without the AppContainer (still in the Job Object,
    // with the restricted environment) instead of a kernel that cannot open its own workspace
    let relaxed = RELAXED.lock().unwrap().as_ref().map(|s| s.contains(&norm(&p.workspace))).unwrap_or(false);
    let mut problem = if relaxed { None } else {
        [&p.workspace, &p.tmp].into_iter().chain(p.write_roots.iter()).chain(p.read_roots.iter()).find_map(|d| volume_problem(d))
    };
    // write access cannot be granted (folder owned by another account / administrators): unisolated rather than no kernel
    if problem.is_none() && !relaxed {
        for d in [&p.workspace, &p.tmp].into_iter().chain(p.write_roots.iter()) {
            if let Err(e) = grant(sid.p, d, true, false) { problem = Some(format!("无法为沙箱授权：{e}")); break; }
        }
    }
    if let Some(why) = &problem {
        if NO_ACL.lock().unwrap().get_or_insert_with(HashSet::new).insert(why.clone()) {
            eprintln!("sandbox: {why}，Windows 沙箱无法访问，改为不隔离运行（{}）", p.workspace.display());
            crate::log::warn("sandbox_off", "", &[("why", why.clone()), ("ws", p.workspace.display().to_string())]);
        }
    }
    let isolated = problem.is_none() && !relaxed;
    for d in p.read_roots.iter().chain([&p.paths.python_root, &p.paths.resources]).filter(|_| isolated) {
        // usually fails under Program Files (not ours to change) and is harmless there (ALL APPLICATION PACKAGES may read):
        // log once per folder, not on every kernel start
        if GRANTED.lock().unwrap().as_ref().map(|s| s.contains(&(d.to_path_buf(), false))).unwrap_or(false) { continue; }
        if let Err(e) = grant(sid.p, d, false, false) {
            let mut g = GRANTED.lock().unwrap();
            if g.get_or_insert_with(HashSet::new).insert((d.to_path_buf(), false)) { eprintln!("sandbox: {e}（只读授权失败；若该目录已对所有应用包可读则无影响）"); }
        }
    }
    let caps: Vec<Sid> = if p.network || !p.db_ports.is_empty() {
        vec![capability(CAP_INTERNET_CLIENT)?, capability(CAP_PRIVATE_NETWORK)?]
    } else { vec![] };
    let mut cap_attrs: Vec<SID_AND_ATTRIBUTES> = caps.iter().map(|c| SID_AND_ATTRIBUTES { Sid: c.p, Attributes: 4 /* SE_GROUP_ENABLED */ }).collect();
    let sc = SECURITY_CAPABILITIES { AppContainerSid: sid.p, Capabilities: if cap_attrs.is_empty() { null_mut() } else { cap_attrs.as_mut_ptr() },
                                     CapabilityCount: cap_attrs.len() as u32, Reserved: 0 };

    let app = wide(&cmd[0]);
    let mut line = wide(&cmd.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let envb = env_block(env);
    let cwdw = wide_path(cwd);
    unsafe {
        let sin = pipe(1)?;
        let sout = pipe(0)?;
        let serr = pipe(0)?;
        let close_all = |hs: &[HANDLE]| for h in hs { CloseHandle(*h); };
        let child_ends = [sin[0], sout[1], serr[1]];

        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size);
        let mut buf = vec![0usize; size / std::mem::size_of::<usize>() + 1];
        let list = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(list, 2, 0, &mut size) == 0 {
            close_all(&[sin[0], sin[1], sout[0], sout[1], serr[0], serr[1]]);
            return Err(format!("InitializeProcThreadAttributeList: {}", last_err()));
        }
        let ok1 = if !isolated { 1 } else { UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize, &sc as *const _ as _,
                                            std::mem::size_of::<SECURITY_CAPABILITIES>(), null_mut(), null()) };
        // only the three pipe ends are inherited, whatever else happens to be inheritable in this process
        let ok2 = UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, child_ends.as_ptr() as _,
                                            std::mem::size_of_val(&child_ends), null_mut(), null());
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = sin[0];
        si.StartupInfo.hStdOutput = sout[1];
        si.StartupInfo.hStdError = serr[1];
        si.lpAttributeList = list;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let created = ok1 != 0 && ok2 != 0 && CreateProcessW(app.as_ptr(), line.as_mut_ptr(), null(), null(), 1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | CREATE_SUSPENDED,
            envb.as_ptr() as _, cwdw.as_ptr(), &si.StartupInfo, &mut pi) != 0;
        let err = last_err();
        DeleteProcThreadAttributeList(list);
        close_all(&child_ends);
        if !created {
            close_all(&[sin[1], sout[0], serr[0]]);
            return Err(format!("无法在沙箱中启动 {}：{err}", cmd[0]));
        }
        let job = CreateJobObjectW(null(), null());
        let mut lim: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        lim.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let jobbed = !job.is_null()
            && SetInformationJobObject(job, JobObjectExtendedLimitInformation, &lim as *const _ as _, std::mem::size_of_val(&lim) as u32) != 0
            && AssignProcessToJobObject(job, pi.hProcess) != 0;
        if !jobbed {
            let e = last_err();
            TerminateProcess(pi.hProcess, 1);
            close_all(&[pi.hProcess, pi.hThread, sin[1], sout[0], serr[0]]);
            if !job.is_null() { CloseHandle(job); }
            return Err(format!("Job Object: {e}"));
        }
        ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);
        let stdin: Option<Box<dyn Write + Send>> = if stdin_piped { Some(Box::new(File::from_raw_handle(sin[1] as _))) } else { CloseHandle(sin[1]); None };
        Ok((Proc { process: pi.hProcess, job, code: None }, stdin,
            Box::new(File::from_raw_handle(sout[0] as _)), Box::new(File::from_raw_handle(serr[0] as _))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting() {
        assert_eq!(quote_arg("abc"), "abc");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r"C:\x y\"), r#""C:\x y\\""#);
        assert_eq!(quote_arg(""), "\"\"");
    }
}
