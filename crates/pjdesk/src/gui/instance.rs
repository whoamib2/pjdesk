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

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\PJDeskClientWindow");
    const EVENT_NAME: windows::core::PCWSTR = w!("Local\\PJDeskClientWindowShow");

    struct Held {
        _mutex: usize,
        event: usize,
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
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(mutex);
                // The new process is in front. Let the open window take the foreground.
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
            *HELD.lock().unwrap() = Some(Held { _mutex: raw(mutex), event: raw(event) });
            true
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
}
