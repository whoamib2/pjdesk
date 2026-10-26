//! One PJDesk window per signed-in Windows user.
//!
//! A second launch asks the first window to show itself and then leaves.
//! It does not open another window, so a locked window cannot be skipped
//! by starting the program again.

/// Takes the window, or signals the owner and returns false.
pub fn acquire_or_signal() -> bool {
    #[cfg(windows)]
    {
        imp::acquire_or_signal()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// True when this process owns the single window.
pub fn holds_window() -> bool {
    #[cfg(windows)]
    {
        imp::holds_window()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// True when another launch asked this process to open the window.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn show_requested() -> bool {
    #[cfg(windows)]
    {
        imp::show_requested()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// True when a window process already holds the single-instance lock.
pub fn gui_is_running() -> bool {
    #[cfg(windows)]
    {
        imp::gui_is_running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Asks the running window to come forward. Does not take the lock.
pub fn signal_show() {
    #[cfg(windows)]
    imp::signal_show();
}

/// Asks the running window to exit. The tray calls this on Quit.
pub fn signal_quit() {
    #[cfg(windows)]
    imp::signal_quit();
}

/// True when the tray asked this window process to exit.
pub fn quit_requested() -> bool {
    #[cfg(windows)]
    {
        imp::quit_requested()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use windows::core::w;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, OpenMutexW, ReleaseMutex, SetEvent, WaitForSingleObject,
        EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\PJDeskClientWindow");
    const EVENT_NAME: windows::core::PCWSTR = w!("Local\\PJDeskClientWindowShow");
    const QUIT_NAME: windows::core::PCWSTR = w!("Local\\PJDeskClientWindowQuit");

    struct Held {
        _mutex: usize,
        event: usize,
        quit: usize,
    }

    fn raw(handle: HANDLE) -> usize {
        handle.0 as usize
    }

    fn handle(raw: usize) -> HANDLE {
        HANDLE(raw as *mut std::ffi::c_void)
    }

    static HELD: Mutex<Option<Held>> = Mutex::new(None);

    pub fn acquire_or_signal() -> bool {
        unsafe {
            let mutex = match CreateMutexW(None, true, MUTEX_NAME) {
                Ok(h) => h,
                Err(_) => return true,
            };
            if GetLastError() == ERROR_ALREADY_EXISTS && !abandoned_or_free(mutex) {
                let _ = CloseHandle(mutex);
                // The new process is in front. Restore the open window here,
                // then let it take the foreground. The window process may not
                // be painting, so the event alone would wait for a frame.
                crate::winshow::show_from_tray("PJDesk");
                let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
                for _ in 0..20 {
                    if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, EVENT_NAME) {
                        let _ = SetEvent(event);
                        let _ = CloseHandle(event);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                tracing::info!("PJDesk is already open. Asked that window to come forward.");
                return false;
            }
            let event = match CreateEventW(None, false, false, EVENT_NAME) {
                Ok(event) => event,
                Err(_) => return true,
            };
            let quit = CreateEventW(None, false, false, QUIT_NAME).map(raw).unwrap_or(0);
            *HELD.lock().unwrap() = Some(Held { _mutex: raw(mutex), event: raw(event), quit });
            true
        }
    }

    /// `ERROR_ALREADY_EXISTS` does not grant ownership. A dead owner leaves
    /// the mutex abandoned, and this wait takes it. A living owner still
    /// holds it, so the wait times out and the caller signals that window.
    fn abandoned_or_free(mutex: HANDLE) -> bool {
        let wait = unsafe { WaitForSingleObject(mutex, 0) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            tracing::info!(
                "The previous PJDesk window is not running. This launch is opening the window. The single-instance lock did not block it."
            );
            true
        } else {
            false
        }
    }

    pub fn holds_window() -> bool {
        HELD.lock().unwrap().is_some()
    }

    pub fn show_requested() -> bool {
        let guard = HELD.lock().unwrap();
        let Some(held) = guard.as_ref() else { return false };
        unsafe { WaitForSingleObject(handle(held.event), 0) == WAIT_OBJECT_0 }
    }

    pub fn quit_requested() -> bool {
        let guard = HELD.lock().unwrap();
        let Some(held) = guard.as_ref() else { return false };
        if held.quit == 0 {
            return false;
        }
        unsafe { WaitForSingleObject(handle(held.quit), 0) == WAIT_OBJECT_0 }
    }

    pub fn gui_is_running() -> bool {
        mutex_held(MUTEX_NAME)
    }

    pub fn signal_show() {
        crate::winshow::show_from_tray("PJDesk");
        let _ = signal_event(EVENT_NAME);
    }

    pub fn signal_quit() {
        let _ = signal_event(QUIT_NAME);
    }

    fn mutex_held(name: windows::core::PCWSTR) -> bool {
        unsafe {
            let Ok(mutex) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, name) else {
                return false;
            };
            let wait = WaitForSingleObject(mutex, 0);
            let held = wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED;
            if !held {
                let _ = ReleaseMutex(mutex);
            }
            let _ = CloseHandle(mutex);
            held
        }
    }

    fn signal_event(name: windows::core::PCWSTR) -> bool {
        unsafe {
            let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, name) else {
                return false;
            };
            let ok = SetEvent(event).is_ok();
            let _ = CloseHandle(event);
            ok
        }
    }
}
