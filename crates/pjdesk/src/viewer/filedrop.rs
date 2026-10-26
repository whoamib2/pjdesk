//! File drops onto the viewer window.
//!
//! Winit's drop target leaves `QueryInterface` unimplemented, so Explorer
//! draws a no-drop cursor and the window never sees the files. This replaces
//! that target on the window's thread, which is already an STA. A viewer that
//! is still elevated (the usual case is not: `connect` relaunches at medium
//! integrity) accepts `WM_DROPFILES` from unelevated Explorer instead.

use std::path::PathBuf;
#[cfg(windows)]
use std::sync::atomic::Ordering;
#[cfg(windows)]
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use windows::core as windows_core;
#[cfg(windows)]
use windows_core::Interface;

#[cfg(windows)]
pub fn attach(window: &winit::window::Window, on_drop: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = windows::Win32::Foundation::HWND(win.hwnd.get() as *mut _);
    let callback: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync> = Arc::new(on_drop);
    {
        let mut slot = ON_DROP.lock().unwrap();
        *slot = Some(callback);
    }
    unsafe { install(hwnd) };
}

#[cfg(not(windows))]
pub fn attach(_window: &winit::window::Window, _on_drop: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) {}

#[cfg(windows)]
static ON_DROP: Mutex<Option<Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>>> = Mutex::new(None);

#[cfg(windows)]
fn deliver(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if let Some(cb) = ON_DROP.lock().unwrap().as_ref() {
        cb(paths);
    }
}

#[cfg(windows)]
unsafe fn install(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::System::Ole::{OleInitialize, RegisterDragDrop, RevokeDragDrop, IDropTarget};
    use windows::Win32::UI::Shell::DragAcceptFiles;

    let _ = OleInitialize(None);
    if crate::platform::process_is_elevated() {
        allow_drop_messages(hwnd);
        DragAcceptFiles(hwnd, true);
        subclass(hwnd);
        tracing::info!("Viewer is elevated; accepting file drops with WM_DROPFILES");
        return;
    }
    let _ = RevokeDragDrop(hwnd);
    let target = Box::new(Target { vtbl: &VTBL, refs: std::sync::atomic::AtomicUsize::new(1) });
    let raw = Box::into_raw(target) as *mut core::ffi::c_void;
    let iface = IDropTarget::from_raw(raw);
    match RegisterDragDrop(hwnd, &iface) {
        Ok(()) => {
            std::mem::forget(iface);
            tracing::info!("Viewer is accepting file drops");
        }
        Err(e) => {
            tracing::warn!("Could not register a file drop target ({e}). Trying WM_DROPFILES.");
            let _ = IDropTarget::from_raw(raw);
            allow_drop_messages(hwnd);
            DragAcceptFiles(hwnd, true);
            subclass(hwnd);
        }
    }
}

#[cfg(windows)]
unsafe fn allow_drop_messages(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, MSGFLT_ALLOW, WM_COPYDATA, WM_DROPFILES};
    let _ = ChangeWindowMessageFilterEx(hwnd, WM_DROPFILES, MSGFLT_ALLOW, None);
    let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
    let _ = ChangeWindowMessageFilterEx(hwnd, 0x0049, MSGFLT_ALLOW, None);
}

#[cfg(windows)]
#[repr(C)]
struct Target {
    vtbl: *const Vtbl,
    refs: std::sync::atomic::AtomicUsize,
}

#[cfg(windows)]
#[repr(C)]
struct Vtbl {
    query: unsafe extern "system" fn(*mut Target, *const windows_core::GUID, *mut *mut core::ffi::c_void) -> windows_core::HRESULT,
    add: unsafe extern "system" fn(*mut Target) -> u32,
    release: unsafe extern "system" fn(*mut Target) -> u32,
    enter: unsafe extern "system" fn(*mut Target, *mut core::ffi::c_void, u32, i64, *mut u32) -> windows_core::HRESULT,
    over: unsafe extern "system" fn(*mut Target, u32, i64, *mut u32) -> windows_core::HRESULT,
    leave: unsafe extern "system" fn(*mut Target) -> windows_core::HRESULT,
    drop: unsafe extern "system" fn(*mut Target, *mut core::ffi::c_void, u32, i64, *mut u32) -> windows_core::HRESULT,
}

#[cfg(windows)]
static VTBL: Vtbl = Vtbl {
    query: query_interface,
    add: add_ref,
    release: release,
    enter: drag_enter,
    over: drag_over,
    leave: drag_leave,
    drop: drop_files,
};

#[cfg(windows)]
const S_OK: windows_core::HRESULT = windows_core::HRESULT(0);
#[cfg(windows)]
const E_NOINTERFACE: windows_core::HRESULT = windows_core::HRESULT(0x8000_4002_u32 as i32);
#[cfg(windows)]
const DROPEFFECT_NONE: u32 = 0;
#[cfg(windows)]
const DROPEFFECT_COPY: u32 = 1;
#[cfg(windows)]
static EFFECT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(DROPEFFECT_NONE);

#[cfg(windows)]
unsafe extern "system" fn query_interface(
    this: *mut Target,
    iid: *const windows_core::GUID,
    out: *mut *mut core::ffi::c_void,
) -> windows_core::HRESULT {
    if iid.is_null() || out.is_null() {
        return E_NOINTERFACE;
    }
    let id = &*iid;
    let known = (id.data1 == 0 || id.data1 == 0x0000_0122)
        && id.data2 == 0
        && id.data3 == 0
        && id.data4 == [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
    if !known {
        *out = std::ptr::null_mut();
        return E_NOINTERFACE;
    }
    *out = this as *mut _;
    add_ref(this);
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn add_ref(this: *mut Target) -> u32 {
    (*this).refs.fetch_add(1, Ordering::Relaxed) as u32 + 1
}

#[cfg(windows)]
unsafe extern "system" fn release(this: *mut Target) -> u32 {
    let left = (*this).refs.fetch_sub(1, Ordering::Relaxed) - 1;
    if left == 0 {
        drop(Box::from_raw(this));
    }
    left as u32
}

#[cfg(windows)]
unsafe extern "system" fn drag_enter(
    _this: *mut Target,
    data: *mut core::ffi::c_void,
    _keys: u32,
    _pt: i64,
    effect: *mut u32,
) -> windows_core::HRESULT {
    let chosen = if has_hdrop(data) { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
    EFFECT.store(chosen, Ordering::Relaxed);
    if !effect.is_null() {
        *effect = chosen;
    }
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn drag_over(_this: *mut Target, _keys: u32, _pt: i64, effect: *mut u32) -> windows_core::HRESULT {
    if !effect.is_null() {
        *effect = EFFECT.load(Ordering::Relaxed);
    }
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn drag_leave(_this: *mut Target) -> windows_core::HRESULT {
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn drop_files(
    _this: *mut Target,
    data: *mut core::ffi::c_void,
    _keys: u32,
    _pt: i64,
    effect: *mut u32,
) -> windows_core::HRESULT {
    let paths = files_of(data);
    if !effect.is_null() {
        *effect = if paths.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY };
    }
    deliver(paths);
    S_OK
}

#[cfg(windows)]
unsafe fn has_hdrop(data: *mut core::ffi::c_void) -> bool {
    use windows::Win32::System::Com::FORMATETC;
    if data.is_null() {
        return false;
    }
    let format = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: 1, lindex: -1, tymed: 1 };
    let vtbl = *(data as *const *const usize);
    if vtbl.is_null() {
        return false;
    }
    type Query = unsafe extern "system" fn(*mut core::ffi::c_void, *const FORMATETC) -> i32;
    let query: Query = std::mem::transmute(*vtbl.add(5));
    query(data, &format) >= 0
}

#[cfg(windows)]
unsafe fn files_of(data: *mut core::ffi::c_void) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::FORMATETC;
    use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};

    if data.is_null() {
        return Vec::new();
    }
    #[repr(C)]
    struct Medium {
        tymed: u32,
        hglobal: *mut core::ffi::c_void,
        punk: *mut core::ffi::c_void,
    }
    let format = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: 1, lindex: -1, tymed: 1 };
    let mut medium = std::mem::zeroed::<Medium>();
    let vtbl = *(data as *const *const usize);
    if vtbl.is_null() {
        return Vec::new();
    }
    type GetData = unsafe extern "system" fn(*mut core::ffi::c_void, *const FORMATETC, *mut Medium) -> i32;
    let get_data: GetData = std::mem::transmute(*vtbl.add(3));
    if get_data(data, &format, &mut medium) < 0 || medium.hglobal.is_null() {
        return Vec::new();
    }
    let hdrop = HDROP(medium.hglobal);
    let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
    let mut out = Vec::new();
    for i in 0..count {
        let len = DragQueryFileW(hdrop, i, None) as usize;
        if len == 0 || len > 32_768 {
            continue;
        }
        let mut buf = vec![0u16; len + 1];
        let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
        if n == 0 {
            continue;
        }
        out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n.min(buf.len())])));
    }
    DragFinish(hdrop);
    out
}

#[cfg(windows)]
static OLD_PROCS: Mutex<Vec<(isize, isize)>> = Mutex::new(Vec::new());

#[cfg(windows)]
unsafe fn subclass(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_WNDPROC};
    let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, drop_proc as *const () as usize as isize);
    if previous != 0 {
        OLD_PROCS.lock().unwrap().push((hwnd.0 as isize, previous));
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{CallWindowProcW, WM_DROPFILES};
    if msg == WM_DROPFILES {
        let hdrop = HDROP(wparam.0 as *mut _);
        let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
        let mut out = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            if len == 0 || len > 32_768 {
                continue;
            }
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            if n > 0 {
                use std::os::windows::ffi::OsStringExt;
                out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n.min(buf.len())])));
            }
        }
        DragFinish(hdrop);
        deliver(out);
        return windows::Win32::Foundation::LRESULT(0);
    }
    let previous = OLD_PROCS.lock().unwrap().iter().find(|(h, _)| *h == hwnd.0 as isize).map(|(_, p)| *p).unwrap_or(0);
    if previous == 0 {
        return windows::Win32::Foundation::LRESULT(0);
    }
    type Proc = unsafe extern "system" fn(
        windows::Win32::Foundation::HWND,
        u32,
        windows::Win32::Foundation::WPARAM,
        windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT;
    let previous: Proc = std::mem::transmute(previous);
    CallWindowProcW(Some(previous), hwnd, msg, wparam, lparam)
}

