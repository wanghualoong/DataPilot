//! Windows implementations (M3 phase C): %APPDATA% layout, WinHTTP / IE proxy incl. PAC, Recycle Bin, time zone.
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinHttp::*;

pub fn app_support_dir() -> PathBuf {
    std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| super::home().join("AppData/Roaming")).join("DataPilot")
}
pub fn python_exe(root: &Path) -> PathBuf { root.join("python.exe") }
pub fn ssh_exe() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32\\OpenSSH\\ssh.exe")
}

pub(crate) fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

/// Takes ownership of a WinHTTP-allocated string (freed with GlobalFree).
unsafe fn take_wstr(p: *mut u16) -> Option<String> {
    if p.is_null() { return None; }
    let mut n = 0; while *p.add(n) != 0 { n += 1; }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
    GlobalFree(p as _);
    Some(s).filter(|s| !s.trim().is_empty())
}

/// "http=a:1;https=b:2" / "a:1" → proxy for https (falls back to http= or the bare entry).
fn pick_proxy(list: &str) -> Option<String> {
    let entries: Vec<&str> = list.split([';', ' ']).map(str::trim).filter(|e| !e.is_empty()).collect();
    let find = |k: &str| entries.iter().find_map(|e| e.strip_prefix(k)).map(str::to_string);
    let p = find("https=").or_else(|| find("http=")).or_else(|| entries.iter().find(|e| !e.contains('=')).map(|e| e.to_string()))?;
    Some(if p.contains("://") { p } else { format!("http://{p}") })
}

fn bypassed(host: &str, bypass: &str) -> bool {
    bypass.split([';', ' ', ',']).map(str::trim).filter(|e| !e.is_empty()).any(|e| {
        if e.eq_ignore_ascii_case("<local>") { return !host.contains('.'); }
        crate::llm::sh_exp_match(host, e) || host.ends_with(e.trim_start_matches('*'))
    })
}

pub fn system_proxy_for(url: &str) -> Option<String> {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':', '?']).next().unwrap_or("").to_string();
    unsafe {
        let mut ie: WINHTTP_CURRENT_USER_IE_PROXY_CONFIG = std::mem::zeroed();
        if WinHttpGetIEProxyConfigForCurrentUser(&mut ie) == 0 { return None; }
        let pac = take_wstr(ie.lpszAutoConfigUrl);
        let proxy = take_wstr(ie.lpszProxy);
        let bypass = take_wstr(ie.lpszProxyBypass).unwrap_or_default();
        if pac.is_some() || ie.fAutoDetect != 0 {
            if let Some(r) = auto_proxy(url, pac.as_deref()) { return r; }
        }
        if bypassed(&host, &bypass) { return None; }
        proxy.as_deref().and_then(pick_proxy)
    }
}

/// PAC / WPAD through WinHTTP. Some(None) = direct, Some(Some(p)) = proxy, None = no answer (fall back to static).
unsafe fn auto_proxy(url: &str, pac: Option<&str>) -> Option<Option<String>> {
    let agent = wide("DataPilot");
    let h = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0);
    if h.is_null() { return None; }
    let pac_w = pac.map(wide);
    let mut opt: WINHTTP_AUTOPROXY_OPTIONS = std::mem::zeroed();
    match &pac_w {
        Some(w) => { opt.dwFlags = WINHTTP_AUTOPROXY_CONFIG_URL; opt.lpszAutoConfigUrl = w.as_ptr(); }
        None => { opt.dwFlags = WINHTTP_AUTOPROXY_AUTO_DETECT; opt.dwAutoDetectFlags = WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A; }
    }
    opt.fAutoLogonIfChallenged = 1;
    let url_w = wide(url);
    let mut info: WINHTTP_PROXY_INFO = std::mem::zeroed();
    let ok = WinHttpGetProxyForUrl(h, url_w.as_ptr(), &mut opt, &mut info) != 0;
    WinHttpCloseHandle(h);
    if !ok { return None; }
    let p = take_wstr(info.lpszProxy);
    let _ = take_wstr(info.lpszProxyBypass);
    Some(if info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY { p.as_deref().and_then(pick_proxy) } else { None })
}

pub fn utc_offset() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        if r == u32::MAX { return 0; }
        let bias = tz.Bias + if r == 2 { tz.DaylightBias } else if r == 1 { tz.StandardBias } else { 0 };   // 2 = TIME_ZONE_ID_DAYLIGHT
        -(bias as i64) * 60
    }
}

pub fn move_to_trash(p: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::*;
    let abs = std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let s = abs.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);   // SHFileOperation does not accept verbatim paths
    let mut from: Vec<u16> = s.encode_utf16().collect();
    from.extend([0, 0]);                            // double-NUL terminated list
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && op.fAnyOperationsAborted == 0 { Ok(()) } else { Err(format!("无法移到回收站（{r}）：{}", p.display())) }
}

/// Put this process into a KILL_ON_JOB_CLOSE job: every child started afterwards (ssh tunnels, Chrome, MCP servers,
/// the kernel's own nested job) dies with the core, even if it crashes. The handle is intentionally never closed.
pub fn contain_children() {
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() { return; }
        let mut lim: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        lim.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &lim as *const _ as _, std::mem::size_of_val(&lim) as u32);
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 { eprintln!("job object: {}", std::io::Error::last_os_error()); }
    }
}

pub fn copy_tree(src: &Path, dest: &Path) -> bool { super::copy_tree_plain(src, dest).is_ok() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_parsing() {
        assert_eq!(pick_proxy("http=a:1;https=b:2").as_deref(), Some("http://b:2"));
        assert_eq!(pick_proxy("10.0.0.1:8080").as_deref(), Some("http://10.0.0.1:8080"));
        assert!(bypassed("intranet", "<local>") && bypassed("x.corp.com", "*.corp.com") && !bypassed("api.openai.com", "*.corp.com"));
    }
}

pub fn free_space(p: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = wide(&p.to_string_lossy());
    let mut avail: u64 = 0;
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}
