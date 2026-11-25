//! Windows folder picker that still lists files (like the macOS panel / Codex): a normal open dialog, so every folder
//! level can be browsed and files are visible, plus a prominent 「选择文件夹：<名字>」 button (IFileDialogCustomize)
//! that picks the folder selected in the list, else the folder being shown. 「打开」 keeps its default meaning (enter
//! the selected folder); opening a file is refused, so only folders can be chosen.
//! (The first version put 「名字」 into the file-name box and decided in OnFileOk: the dialog then entered the selected
//! folder instead of choosing it, and in practice only the first level could be picked.)
use std::sync::{Arc, Mutex};
use windows::core::{implement, Error, Interface, Ref, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, S_FALSE, S_OK};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER, SFGAO_STREAM};
use windows::Win32::UI::Shell::*;

const BTN: u32 = 1001;

#[implement(IFileDialogEvents, IFileDialogControlEvents)]
struct Events { picked: Arc<Mutex<Option<String>>>, label: String }

fn pwstr(p: windows::core::PWSTR) -> String {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s
}

/// File-system path of a real folder (zip files report FOLDER | STREAM: treated as files).
fn folder_path(item: &IShellItem) -> Option<String> {
    let a = unsafe { item.GetAttributes(SFGAO_FOLDER | SFGAO_STREAM | SFGAO_FILESYSTEM) }.ok()?;
    if a.0 & SFGAO_FOLDER.0 == 0 || a.0 & SFGAO_FILESYSTEM.0 == 0 || a.0 & SFGAO_STREAM.0 != 0 { return None; }
    unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok().map(pwstr).filter(|p| !p.is_empty())
}

/// What the button would pick: the folder selected in the list, else the folder being shown.
fn target(d: &IFileDialog) -> Option<String> {
    unsafe { d.GetCurrentSelection() }.ok().and_then(|i| folder_path(&i))
        .or_else(|| unsafe { d.GetFolder() }.ok().and_then(|i| folder_path(&i)))
}

fn short_name(path: &str) -> String {
    let t = path.trim_end_matches(['\\', '/']);
    let name = t.rsplit(['\\', '/']).next().filter(|n| !n.is_empty() && !n.ends_with(':')).unwrap_or(path);
    let mut s: String = name.chars().take(40).collect();
    if name.chars().count() > 40 { s.push('…'); }
    s
}

impl Events_Impl {
    fn relabel(&self, d: &IFileDialog) {
        let Ok(c) = d.cast::<IFileDialogCustomize>() else { return };
        let text = match target(d) { Some(p) => format!("{}：{}", self.label, short_name(&p)), None => self.label.clone() };
        let h = HSTRING::from(text.as_str());
        let _ = unsafe { c.SetControlLabel(BTN, PCWSTR(h.as_ptr())) };
    }
}

impl IFileDialogEvents_Impl for Events_Impl {
    /// 「打开」 on a file: refused (dialog stays open). On a folder the dialog normally enters it instead of getting here.
    fn OnFileOk(&self, pfd: Ref<IFileDialog>) -> Result<()> {
        let d = pfd.ok()?;
        match unsafe { d.GetResult() }.ok().and_then(|i| folder_path(&i)) {
            Some(p) => { *self.picked.lock().unwrap() = Some(p); Ok(()) }
            None => Err(Error::from_hresult(S_FALSE)),
        }
    }
    fn OnFolderChanging(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnFolderChange(&self, pfd: Ref<IFileDialog>) -> Result<()> { if let Ok(d) = pfd.ok() { self.relabel(d); } Ok(()) }
    fn OnSelectionChange(&self, pfd: Ref<IFileDialog>) -> Result<()> { if let Ok(d) = pfd.ok() { self.relabel(d); } Ok(()) }
    fn OnShareViolation(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_SHAREVIOLATION_RESPONSE> { Ok(FDESVR_DEFAULT) }
    fn OnTypeChange(&self, _: Ref<IFileDialog>) -> Result<()> { Ok(()) }
    fn OnOverwrite(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_OVERWRITE_RESPONSE> { Ok(FDEOR_DEFAULT) }
}

impl IFileDialogControlEvents_Impl for Events_Impl {
    fn OnItemSelected(&self, _: Ref<IFileDialogCustomize>, _: u32, _: u32) -> Result<()> { Ok(()) }
    fn OnButtonClicked(&self, pfdc: Ref<IFileDialogCustomize>, id: u32) -> Result<()> {
        if id != BTN { return Ok(()); }
        let d: IFileDialog = pfdc.ok()?.cast()?;
        if let Some(p) = target(&d) {
            *self.picked.lock().unwrap() = Some(p);
            unsafe { d.Close(S_OK) }?;
        }
        Ok(())
    }
    fn OnCheckButtonToggled(&self, _: Ref<IFileDialogCustomize>, _: u32, _: windows::core::BOOL) -> Result<()> { Ok(()) }
    fn OnControlActivating(&self, _: Ref<IFileDialogCustomize>, _: u32) -> Result<()> { Ok(()) }
}

/// Modal folder picker on its own STA thread. `owner`: raw HWND of the main window (0 = none). None = cancelled.
/// `ok` is the button text (「选择文件夹」); `field` is unused (kept for the command signature).
pub fn pick(owner: isize, title: String, ok: String, field: String, start: Option<String>) -> Option<String> {
    let _ = field;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = (|| -> Result<Option<String>> {
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let o = d.GetOptions()?;
            d.SetOptions(FILEOPENDIALOGOPTIONS((o.0 | FOS_FORCEFILESYSTEM.0 | FOS_PATHMUSTEXIST.0 | FOS_NOCHANGEDIR.0) & !FOS_ALLOWMULTISELECT.0))?;
            let t = HSTRING::from(title.as_str());
            d.SetTitle(PCWSTR(t.as_ptr()))?;
            let c: IFileDialogCustomize = d.cast()?;
            let k = HSTRING::from(ok.as_str());
            c.AddPushButton(BTN, PCWSTR(k.as_ptr()))?;
            let _ = c.MakeProminent(BTN);
            if let Some(s) = start.filter(|s| std::path::Path::new(s).is_dir()) {
                let hs = HSTRING::from(s.as_str());
                if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(hs.as_ptr()), None) { let _ = d.SetFolder(&item); }
            }
            let picked = Arc::new(Mutex::new(None));
            let ev: IFileDialogEvents = Events { picked: picked.clone(), label: ok.clone() }.into();
            let cookie = d.Advise(&ev)?;
            let shown = d.Show(if owner != 0 { Some(HWND(owner as _)) } else { None });
            let _ = d.Unadvise(cookie);
            let p = picked.lock().unwrap().take();
            if p.is_some() || shown.is_err() { return Ok(p); }
            Ok(d.GetResult().ok().and_then(|i| folder_path(&i)))
        })();
        CoUninitialize();
        r.ok().flatten()
    }).join().ok().flatten()
}
