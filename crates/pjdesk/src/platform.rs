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

/// Windows session this process is running in. Session 0 is the service
/// session and has no interactive desktop, so capture there stays black.
#[cfg(windows)]
pub fn session_id() -> Option<u32> {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    unsafe {
        let mut id = 0u32;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut id).ok()?;
        Some(id)
    }
}

#[cfg(not(windows))]
pub fn session_id() -> Option<u32> {
    None
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

/// Relaunches elevated with `args`, passing `password` through a file in the
/// caller's profile instead of the command line. The command line of an
/// elevated process is visible to other local users and to audit logs.
#[cfg(windows)]
pub fn run_elevated_with_password(args: &[&str], password: &str) -> bool {
    let name = format!("pjdesk-pw-{}.txt", hex::encode(pjdesk_proto::random_bytes::<8>()));
    let path = std::env::temp_dir().join(name);
    if std::fs::write(&path, password).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let path_str = path.display().to_string();
    let mut owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    owned.push("--password-file".into());
    owned.push(path_str);
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let ok = run_elevated(&refs);
    if !ok {
        let _ = std::fs::remove_file(&path);
        return false;
    }
    // ShellExecute returns as soon as the UAC prompt appears. If the prompt is
    // dismissed, nothing else deletes the file, so this process does — after
    // long enough for an accepted prompt to have read it.
    let cleanup = path;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(10 * 60));
        let _ = std::fs::remove_file(cleanup);
    });
    true
}

#[cfg(not(windows))]
pub fn run_elevated_with_password(_args: &[&str], _password: &str) -> bool {
    false
}

/// Reads a password file passed to an elevated helper and deletes it.
pub fn read_password_file(path: &std::path::Path) -> anyhow::Result<String> {
    let pw = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("could not read the password file: {e}"))?;
    let _ = std::fs::remove_file(path);
    let pw = pw.trim_end_matches(['\r', '\n']).to_string();
    if pw.is_empty() {
        anyhow::bail!("the password file was empty");
    }
    Ok(pw)
}

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

/// Locks the Windows session. Returns false when the call is not available.
#[cfg(windows)]
pub fn lock_workstation() -> bool {
    use windows::Win32::System::Shutdown::LockWorkStation;
    unsafe { LockWorkStation().is_ok() }
}

#[cfg(not(windows))]
pub fn lock_workstation() -> bool {
    false
}

/// Blocks or unblocks the local keyboard and mouse. Needs a privileged host.
#[cfg(windows)]
pub fn set_block_input(block: bool) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::BlockInput;
    unsafe { BlockInput(block).is_ok() }
}

#[cfg(not(windows))]
pub fn set_block_input(_block: bool) -> bool {
    false
}

/// Asks Windows to restart in a few seconds. The installer/session is not
/// involved; this is the same restart a person would request from the Start menu.
#[cfg(windows)]
pub fn restart_windows() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("shutdown")
        .args(["/r", "/t", "5", "/c", "PJDesk remote restart"])
        .creation_flags(0x0800_0000)
        .spawn()
        .is_ok()
}

#[cfg(not(windows))]
pub fn restart_windows() -> bool {
    false
}

/// Covers every monitor with a black topmost window while `on` is true.
/// The window is created on a background thread and closed when privacy mode ends.
pub fn set_privacy_screen(on: bool) {
    static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    ON.store(on, std::sync::atomic::Ordering::SeqCst);
    if on {
        std::thread::Builder::new().name("privacy".into()).spawn(move || privacy_thread(&ON)).ok();
    }
}

fn privacy_thread(flag: &std::sync::atomic::AtomicBool) {
    #[cfg(windows)]
    {
        privacy_window(flag);
    }
    #[cfg(not(windows))]
    {
        while flag.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

#[cfg(windows)]
fn privacy_window(flag: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("PJDeskPrivacy");
        let _ = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(privacy_wndproc),
            hInstance: instance.into(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: class,
            ..Default::default()
        });
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let wdt = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let hgt = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("PJDesk"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            wdt,
            hgt,
            None,
            None,
            Some(instance.into()),
            None,
        );
        let Ok(hwnd) = hwnd else { return };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, wdt, hgt, SWP_SHOWWINDOW);
        let _ = CreateSolidBrush; // keep the import used if the stock brush is enough
        while flag.load(Ordering::SeqCst) {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(windows)]
unsafe extern "system" fn privacy_wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Graphics::Gdi::{DrawTextW, GetDC, ReleaseDC, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, TRANSPARENT};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::*;
    if msg == WM_PAINT {
        unsafe {
            let dc = GetDC(Some(hwnd));
            if !dc.is_invalid() {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = SetBkMode(dc, TRANSPARENT);
                let _ = SetTextColor(dc, windows::Win32::Foundation::COLORREF(0x00_e8_ec_f2));
                let mut text: Vec<u16> = "PJDesk privacy mode — a remote session is active\0".encode_utf16().collect();
                let _ = DrawTextW(dc, &mut text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                let _ = ReleaseDC(Some(hwnd), dc);
            }
        }
        return windows::Win32::Foundation::LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
