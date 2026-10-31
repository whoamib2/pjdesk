//! Opens Explorer on received files with them selected.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Collects files saved during one batch and shows them once the batch is over.
#[derive(Default)]
pub struct Batch {
    saved: Vec<PathBuf>,
    last: Option<Instant>,
}

impl Batch {
    pub fn saved(&mut self, path: PathBuf) {
        if !self.saved.contains(&path) {
            self.saved.push(path);
        }
        self.last = Some(Instant::now());
    }

    /// When nothing is still arriving and the last file landed a moment ago,
    /// returns the batch and starts a new one.
    pub fn take_if_settled(&mut self, still_arriving: bool) -> Option<Vec<PathBuf>> {
        if still_arriving || self.saved.is_empty() {
            return None;
        }
        if self.last.map(|t| t.elapsed() < Duration::from_millis(800)).unwrap_or(false) {
            return None;
        }
        self.last = None;
        Some(std::mem::take(&mut self.saved))
    }
}

/// Opens one Explorer window on the folder of these files with them selected.
/// Runs on its own thread, so it can be called from anywhere.
pub fn select_in_explorer(paths: Vec<PathBuf>) {
    if paths.is_empty() || cfg!(test) {
        return;
    }
    tracing::info!("Showing {} received file(s) in Explorer", paths.len());
    let _ = std::thread::Builder::new().name("reveal".into()).spawn(move || imp::select(paths));
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILFindLastID, SHOpenFolderAndSelectItems, SHParseDisplayName};

    fn wide(p: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn select(paths: Vec<PathBuf>) {
        let Some(folder) = paths[0].parent().map(|p| p.to_path_buf()) else { return };
        let here: Vec<PathBuf> = paths.iter().filter(|p| p.parent() == Some(folder.as_path())).cloned().collect();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut folder_id: *mut ITEMIDLIST = std::ptr::null_mut();
            let fw = wide(&folder);
            if SHParseDisplayName(PCWSTR(fw.as_ptr()), None, &mut folder_id, 0, None).is_ok() && !folder_id.is_null() {
                let mut full: Vec<*mut ITEMIDLIST> = Vec::new();
                for p in &here {
                    let w = wide(p);
                    let mut id: *mut ITEMIDLIST = std::ptr::null_mut();
                    if SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut id, 0, None).is_ok() && !id.is_null() {
                        full.push(id);
                    }
                }
                let children: Vec<*const ITEMIDLIST> = full.iter().map(|id| ILFindLastID(*id) as *const ITEMIDLIST).collect();
                let ok = SHOpenFolderAndSelectItems(folder_id, Some(&children), 0).is_ok();
                for id in full {
                    CoTaskMemFree(Some(id as *const _));
                }
                CoTaskMemFree(Some(folder_id as *const _));
                if ok {
                    CoUninitialize();
                    return;
                }
            }
            CoUninitialize();
        }
        // Fallback: Explorer selects the first file.
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(paths[0].as_os_str());
        let _ = std::process::Command::new("explorer.exe").arg(arg).spawn();
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn select(paths: Vec<std::path::PathBuf>) {
        if let Some(dir) = paths[0].parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_is_shown_once_nothing_is_arriving() {
        let mut b = Batch::default();
        assert!(b.take_if_settled(false).is_none());
        b.saved(PathBuf::from("/d/a.txt"));
        b.saved(PathBuf::from("/d/b.txt"));
        assert!(b.take_if_settled(true).is_none(), "still arriving");
        assert!(b.take_if_settled(false).is_none(), "too soon");
        b.last = Some(Instant::now() - Duration::from_secs(2));
        assert_eq!(b.take_if_settled(false).unwrap().len(), 2);
        assert!(b.take_if_settled(false).is_none());
    }
}
