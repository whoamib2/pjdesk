//! Low-level keyboard capture for the Windows viewer.
//!
//! While the remote session window is in front, a `WH_KEYBOARD_LL` hook on a
//! dedicated thread (with its own message loop, never the UI thread) swallows
//! keys so Windows does not act on them locally, and forwards them to the host
//! with the original scan code and extended flag. The hook never touches the
//! local mouse.
//!
//! Escape: open the toolbar with the mouse (keys are going to the remote, so a
//! keyboard shortcut would be stolen). The Keys menu can turn capture off, and
//! still sends Ctrl+Alt+Del (Windows will not let a hook capture that chord).
//! Ctrl+Alt+End also sends Ctrl+Alt+Del, as in Remote Desktop.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};


use pjdesk_proto::peer::{ControlMsg, InputEvent};
#[cfg(windows)]
use pjdesk_proto::peer::SpecialCommand;
use tokio::sync::mpsc;

/// Scan-code bit the host already uses for an extended (0xE0) key.
pub const EXTENDED: u16 = 0xE000;

/// A running hook, or a no-op stand-in on non-Windows builds.
pub struct SessionHook {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    enabled: AtomicBool,
    swallow: AtomicBool,
    android_remote: AtomicBool,
    control: Mutex<mpsc::UnboundedSender<ControlMsg>>,
    hwnds: Mutex<Vec<isize>>,
    pressed: Mutex<HashSet<u16>>,
    /// Hook thread id, for WM_QUIT. 0 until the thread starts.
    tid: AtomicU32,
    stop: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookKey {
    pub scan: u16,
    pub down: bool,
    pub vk: u32,
}

pub fn hwnd_of(_window: &winit::window::Window) -> Option<isize> {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = _window.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as isize),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Pack a low-level hook event into the scan code the host expects.
pub fn pack_scan(scan: u32, extended: bool) -> u16 {
    let mut code = (scan & 0xFF) as u16;
    if code == 0 {
        return 0;
    }
    if extended {
        code |= EXTENDED;
    }
    code
}

/// Keys Windows would handle itself (Start, Alt+Tab, and so on) even if this
/// window is in front. Used when the remote is Android and ordinary typing
/// must still reach the window so it can go out as text.
pub fn is_system_vk(vk: u32, alt_down: bool, ctrl_down: bool) -> bool {
    const VK_TAB: u32 = 0x09;
    const VK_ESCAPE: u32 = 0x1B;
    const VK_F4: u32 = 0x73;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const VK_APPS: u32 = 0x5D;
    const VK_SNAPSHOT: u32 = 0x2C;
    const VK_F10: u32 = 0x79;
    const VK_LMENU: u32 = 0xA4;
    const VK_RMENU: u32 = 0xA5;
    const VK_MENU: u32 = 0x12;
    matches!(
        vk,
        VK_LWIN | VK_RWIN | VK_APPS | VK_SNAPSHOT | VK_F10 | VK_LMENU | VK_RMENU | VK_MENU
    ) || (alt_down && matches!(vk, VK_TAB | VK_ESCAPE | VK_F4))
        || (ctrl_down && vk == VK_ESCAPE)
}

/// Ctrl+Alt+End, the Remote Desktop chord that sends Ctrl+Alt+Del.
pub fn is_sas_vk(vk: u32, down: bool, ctrl: bool, alt: bool) -> bool {
    const VK_END: u32 = 0x23;
    down && ctrl && alt && vk == VK_END
}

impl SessionHook {
    pub fn start(control: mpsc::UnboundedSender<ControlMsg>) -> Self {
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(true),
            swallow: AtomicBool::new(false),
            android_remote: AtomicBool::new(false),
            control: Mutex::new(control),
            hwnds: Mutex::new(Vec::new()),
            pressed: Mutex::new(HashSet::new()),
            tid: AtomicU32::new(0),
            stop: AtomicBool::new(false),
        });
        let thread = start_thread(shared.clone());
        Self { shared, thread }
    }

    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::SeqCst)
    }

    pub fn set_enabled(&self, on: bool) {
        self.shared.enabled.store(on, Ordering::SeqCst);
        if !on {
            self.release_pressed();
        }
    }

    pub fn toggle(&self) -> bool {
        let on = !self.enabled();
        self.set_enabled(on);
        on
    }

    pub fn set_swallow(&self, on: bool) {
        let was = self.shared.swallow.swap(on, Ordering::SeqCst);
        if was && !on {
            self.release_pressed();
        }
    }

    pub fn set_android_remote(&self, on: bool) {
        self.shared.android_remote.store(on, Ordering::SeqCst);
    }

    pub fn set_control(&self, control: mpsc::UnboundedSender<ControlMsg>) {
        if let Ok(mut slot) = self.shared.control.lock() {
            *slot = control;
        }
    }

    pub fn set_hwnds(&self, hwnds: Vec<isize>) {
        if let Ok(mut slot) = self.shared.hwnds.lock() {
            *slot = hwnds;
        }
    }

    /// The hook already sent ReleaseAll, or the viewer did. Forget held keys.
    pub fn clear_pressed(&self) {
        if let Ok(mut slot) = self.shared.pressed.lock() {
            slot.clear();
        }
    }

    pub fn release_pressed(&self) {
        let had = self
            .shared
            .pressed
            .lock()
            .map(|mut slot| {
                let any = !slot.is_empty();
                slot.clear();
                any
            })
            .unwrap_or(false);
        if had {
            send_msg(&self.shared, ControlMsg::Input(InputEvent::ReleaseAll));
        }
    }

    pub fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.release_pressed();
        stop_thread(&self.shared);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SessionHook {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_msg(shared: &Shared, msg: ControlMsg) {
    if let Ok(tx) = shared.control.lock() {
        let _ = tx.send(msg);
    }
}

fn start_thread(_shared: Arc<Shared>) -> Option<std::thread::JoinHandle<()>> {
    #[cfg(windows)]
    {
        let shared = _shared;
        match std::thread::Builder::new().name("pjdesk-keyhook".into()).spawn(move || imp::run(shared)) {
            Ok(thread) => Some(thread),
            Err(err) => {
                tracing::warn!("The keyboard hook thread did not start: {err}");
                None
            }
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn stop_thread(_shared: &Shared) {
    #[cfg(windows)]
    {
        imp::request_stop(_shared);
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG,
        PostThreadMessageW, WH_KEYBOARD_LL, WM_QUIT,
    };

    static ACTIVE: Mutex<Option<Arc<Shared>>> = Mutex::new(None);

    pub fn run(shared: Arc<Shared>) {
        let tid = unsafe { GetCurrentThreadId() };
        shared.tid.store(tid, Ordering::SeqCst);
        if let Ok(mut slot) = ACTIVE.lock() {
            *slot = Some(shared.clone());
        }
        let hook = unsafe {
            let module = GetModuleHandleW(None).ok().map(|handle| HINSTANCE(handle.0));
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(proc),
                module,
                0,
            )
        };
        let hook = match hook {
            Ok(hook) => hook,
            Err(err) => {
                tracing::warn!("The keyboard hook was not installed: {err}");
                if let Ok(mut slot) = ACTIVE.lock() {
                    *slot = None;
                }
                return;
            }
        };
        tracing::info!("Keyboard capture is on. Keys go to the remote while this window is in front. Open the toolbar with the mouse to turn it off or end the session.");
        unsafe {
            let mut msg = MSG::default();
            while !shared.stop.load(Ordering::SeqCst) {
                let got = GetMessageW(&mut msg, None, 0, 0);
                if got.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
        }
        if let Ok(mut slot) = ACTIVE.lock() {
            *slot = None;
        }
        shared.tid.store(0, Ordering::SeqCst);
    }

    pub fn request_stop(shared: &Shared) {
        let tid = shared.tid.load(Ordering::SeqCst);
        if tid != 0 {
            let _ = unsafe { PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }

    fn ours(shared: &Shared) -> bool {
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() {
            return false;
        }
        let raw = fg.0 as isize;
        shared
            .hwnds
            .lock()
            .ok()
            .is_some_and(|list| list.iter().any(|hwnd| *hwnd == raw))
    }

    fn modifiers() -> (bool, bool) {
        // High bit set means the key is down now.
        unsafe {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            let alt = GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0;
            (ctrl, alt)
        }
    }

    fn handle(shared: &Arc<Shared>, info: &KBDLLHOOKSTRUCT) -> bool {
        if !shared.enabled.load(Ordering::SeqCst) {
            return false;
        }
        if !ours(shared) {
            shared.release_pressed_inner();
            return false;
        }
        if !shared.swallow.load(Ordering::SeqCst) {
            return false;
        }
        if info.flags.contains(LLKHF_INJECTED) {
            return false;
        }
        let down = !info.flags.contains(LLKHF_UP);
        let extended = info.flags.contains(LLKHF_EXTENDED);
        let vk = info.vkCode;
        let (ctrl, alt) = modifiers();
        if is_sas_vk(vk, down, ctrl, alt) {
            if down {
                send_msg(shared, ControlMsg::Special(SpecialCommand::CtrlAltDel));
            }
            return true;
        }
        if shared.android_remote.load(Ordering::SeqCst) && !is_system_vk(vk, alt, ctrl) {
            return false;
        }
        let scan = pack_scan(info.scanCode, extended);
        if scan == 0 {
            return true;
        }
        if let Ok(mut pressed) = shared.pressed.lock() {
            if down {
                pressed.insert(scan);
            } else {
                pressed.remove(&scan);
            }
        }
        send_msg(shared, ControlMsg::Input(InputEvent::Key { scan, down }));
        true
    }

    unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && lparam.0 != 0 {
            if let Ok(slot) = ACTIVE.lock() {
                if let Some(shared) = slot.as_ref() {
                    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                    if handle(shared, info) {
                        return LRESULT(1);
                    }
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    impl Shared {
        fn release_pressed_inner(&self) {
            let had = self
                .pressed
                .lock()
                .map(|mut slot| {
                    let any = !slot.is_empty();
                    slot.clear();
                    any
                })
                .unwrap_or(false);
            if had {
                send_msg(self, ControlMsg::Input(InputEvent::ReleaseAll));
            }
        }
    }
}

// release_pressed is called from Shared on Windows via inherent impl in imp.
// The method on SessionHook also exists for the viewer.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_scan_sets_the_extended_bit() {
        assert_eq!(pack_scan(0x1D, false), 0x1D);
        assert_eq!(pack_scan(0x5B, true), 0xE05B);
        assert_eq!(pack_scan(0x37, true), 0xE037);
        assert_eq!(pack_scan(0, true), 0);
    }

    #[test]
    fn system_keys_include_win_alt_tab_and_print_screen() {
        assert!(is_system_vk(0x5B, false, false));
        assert!(is_system_vk(0x5C, false, false));
        assert!(is_system_vk(0x5D, false, false));
        assert!(is_system_vk(0x2C, false, false));
        assert!(is_system_vk(0x09, true, false));
        assert!(is_system_vk(0x1B, true, false));
        assert!(is_system_vk(0x73, true, false));
        assert!(is_system_vk(0x1B, false, true));
        assert!(!is_system_vk(0x41, false, false));
        assert!(!is_system_vk(0x09, false, false));
    }

    #[test]
    fn sas_is_ctrl_alt_end_on_the_way_down() {
        assert!(is_sas_vk(0x23, true, true, true));
        assert!(!is_sas_vk(0x23, false, true, true));
        assert!(!is_sas_vk(0x23, true, true, false));
        assert!(!is_sas_vk(0x2E, true, true, true));
    }
}
