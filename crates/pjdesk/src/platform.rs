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
    use std::cell::Cell;
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
        DESKTOP_CREATEWINDOW, DESKTOP_JOURNALPLAYBACK, DESKTOP_JOURNALRECORD, DESKTOP_READOBJECTS,
        DESKTOP_WRITEOBJECTS, HDESK,
    };
    // The desktop this thread previously opened (0 = none). SetThreadDesktop
    // does not free it, so we close it ourselves after switching away.
    thread_local! { static PREV: Cell<isize> = const { Cell::new(0) }; }

    let access = DESKTOP_ACCESS_FLAGS(
        DESKTOP_READOBJECTS.0
            | DESKTOP_WRITEOBJECTS.0
            | DESKTOP_CREATEWINDOW.0
            | DESKTOP_JOURNALRECORD.0
            | DESKTOP_JOURNALPLAYBACK.0,
    );
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), true, access) {
            Ok(hdesk) => {
                if SetThreadDesktop(hdesk).is_ok() {
                    // We've switched off the previous desktop, so it's safe to
                    // close our old handle (closing never destroys a desktop
                    // other handles still reference).
                    PREV.with(|p| {
                        let old = p.replace(hdesk.0 as isize);
                        if old != 0 {
                            let _ = CloseDesktop(HDESK(old as *mut core::ffi::c_void));
                        }
                    });
                    true
                } else {
                    let _ = CloseDesktop(hdesk);
                    false
                }
            }
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
    let params = args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ");
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

/// Injects the secure attention sequence (Ctrl+Alt+Del) via `sas.dll`. This
/// only works when the caller runs with enough privilege — i.e. the background
/// service — so it's best effort. The DLL is loaded dynamically because it
/// isn't present on every SKU.
#[cfg(windows)]
pub fn send_ctrl_alt_del() {
    use windows::core::s;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    unsafe {
        let Ok(lib) = LoadLibraryA(s!("sas.dll")) else {
            tracing::warn!("Ctrl+Alt+Del: sas.dll not available");
            return;
        };
        if let Some(proc) = GetProcAddress(lib, s!("SendSAS")) {
            // void SendSAS(BOOL AsUser)
            let send: unsafe extern "system" fn(i32) = std::mem::transmute(proc);
            send(0); // AsUser = FALSE: works from a service/SYSTEM session
        } else {
            tracing::warn!("Ctrl+Alt+Del: SendSAS not found in sas.dll");
        }
    }
}

#[cfg(not(windows))]
pub fn send_ctrl_alt_del() {}

/// Quotes one argument the way `CommandLineToArgvW` parses it, so a value with
/// spaces, quotes or trailing backslashes (e.g. a password) survives intact.
#[cfg(windows)]
fn quote_arg(s: &str) -> String {
    if !s.is_empty() && !s.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | b'"')) {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::from("\"");
    let mut i = 0;
    while i < chars.len() {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // Escape all backslashes that precede the closing quote.
            for _ in 0..backslashes * 2 {
                out.push('\\');
            }
        } else if chars[i] == '"' {
            for _ in 0..backslashes * 2 + 1 {
                out.push('\\');
            }
            out.push('"');
            i += 1;
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push(chars[i]);
            i += 1;
        }
    }
    out.push('"');
    out
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
