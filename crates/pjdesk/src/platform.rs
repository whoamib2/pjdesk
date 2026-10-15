//! Small platform helpers for the background service: following the *input
//! desktop* so the capture and input threads can see UAC prompts, the lock
//! screen and the sign-in screen (each of which lives on its own desktop).
//!
//! On non-Windows these are no-ops.

/// Returns the name of the desktop currently receiving input ("Default",
/// "Winlogon", "Screen-saver"…), or `None` if it can't be read.
#[cfg(windows)]
pub fn input_desktop_name() -> Option<String> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
        DESKTOP_READOBJECTS, UOI_NAME,
    };
    unsafe {
        let hdesk = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS).ok()?;
        let mut buf = [0u16; 256];
        let mut needed = 0u32;
        let ok = GetUserObjectInformationW(
            HANDLE(hdesk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        );
        let _ = CloseDesktop(hdesk);
        let _ = DESKTOP_ACCESS_FLAGS(0);
        ok.ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Attaches the current thread to the input desktop, so GDI/DXGI capture and
/// `SendInput` act on whatever is on screen now (including the secure desktop).
/// Returns true on success.
#[cfg(windows)]
pub fn attach_thread_to_input_desktop() -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEWINDOW,
        DESKTOP_JOURNALPLAYBACK, DESKTOP_JOURNALRECORD, DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS,
    };
    let access = DESKTOP_ACCESS_FLAGS(
        DESKTOP_READOBJECTS.0
            | DESKTOP_WRITEOBJECTS.0
            | DESKTOP_CREATEWINDOW.0
            | DESKTOP_JOURNALRECORD.0
            | DESKTOP_JOURNALPLAYBACK.0,
    );
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), true, access) {
            // Ownership of hdesk passes to the thread; the old one is released
            // by the OS when the thread exits or attaches to another desktop.
            Ok(hdesk) => SetThreadDesktop(hdesk).is_ok(),
            Err(_) => false,
        }
    }
}

/// Relaunches this program elevated (UAC "Run as administrator") with the given
/// arguments, in a visible console window. Returns true if the prompt launched.
/// Used by the GUI for the service install/uninstall/configure steps.
#[cfg(windows)]
pub fn run_elevated(args: &[&str]) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let Ok(exe) = std::env::current_exe() else { return false };
    let params = args
        .iter()
        .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.to_string() })
        .collect::<Vec<_>>()
        .join(" ");
    unsafe {
        let hinst = ShellExecuteW(
            None,
            &HSTRING::from("runas"),
            &HSTRING::from(exe.as_os_str()),
            &HSTRING::from(params),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // ShellExecute returns > 32 on success.
        hinst.0 as isize > 32
    }
}

#[cfg(not(windows))]
pub fn run_elevated(_args: &[&str]) -> bool {
    false
}

#[cfg(not(windows))]
pub fn input_desktop_name() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn attach_thread_to_input_desktop() -> bool {
    false
}
