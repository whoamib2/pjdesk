//! Hostname, operating system, and the last person signed in at this computer.
//!
//! The last user is remembered when the console is locked or nobody is signed
//! in, so the account list does not go blank. The name is reported only after
//! the server asks, and only the signed-in account owner can read it back.

use std::path::Path;

use pjdesk_proto::signal::ClientMsg;

/// `DOMAIN\user`, or just `user` when there is no domain or it is this computer.
pub fn format_console_user(domain: &str, user: &str, computer: &str) -> String {
    let user = sanitize(user, 80);
    if user.is_empty() {
        return String::new();
    }
    let domain = sanitize(domain, 80);
    if domain.is_empty() || domain.eq_ignore_ascii_case(computer.trim()) {
        user
    } else {
        format!("{domain}\\{user}")
    }
}

/// Keeps `live` when someone is signed in, and the previous name when not.
pub fn remember_console_user(path: &Path, live: &str) -> String {
    let live = sanitize(live, 80);
    if !live.is_empty() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, &live);
        return live;
    }
    std::fs::read_to_string(path).ok().map(|s| sanitize(&s, 80)).unwrap_or_default()
}

pub fn report() -> ClientMsg {
    let (os_family, os_version) = os_facts();
    ClientMsg::ReportHost {
        hostname: crate::hostname(),
        os_family,
        os_version,
        console_user: console_user(),
    }
}

fn console_user() -> String {
    let path = crate::config::Config::path().with_file_name("console-user.txt");
    remember_console_user(&path, &live_console_user())
}

fn sanitize(text: &str, max: usize) -> String {
    text.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

#[cfg(windows)]
fn os_facts() -> (String, String) {
    use windows::core::w;
    let build = reg_sz(w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), w!("CurrentBuildNumber"));
    let display = reg_sz(w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), w!("DisplayVersion")).unwrap_or_default();
    let number = build.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let release = if number >= 22000 {
        "11"
    } else if number > 0 {
        "10"
    } else {
        ""
    };
    let version = if display.is_empty() {
        release.to_string()
    } else if release.is_empty() {
        display
    } else {
        format!("{release} {display}")
    };
    ("windows".into(), version)
}

#[cfg(windows)]
fn reg_sz(subkey: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<String> {
    use windows::Win32::Foundation::NO_ERROR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    unsafe {
        let mut bytes = 0u32;
        let _ = RegGetValueW(HKEY_LOCAL_MACHINE, subkey, value, RRF_RT_REG_SZ, None, None, Some(&mut bytes));
        if bytes < 2 || bytes > 512 {
            return None;
        }
        let mut buf = vec![0u8; bytes as usize];
        let err = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        );
        if err != NO_ERROR {
            return None;
        }
        let n = (bytes as usize).min(buf.len()) / 2;
        let units: Vec<u16> = buf[..n * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
        let text = String::from_utf16_lossy(&units);
        let text = sanitize(&text, 40);
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(windows)]
fn live_console_user() -> String {
    use windows::Win32::System::RemoteDesktop::{WTSDomainName, WTSGetActiveConsoleSessionId, WTSUserName};
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        if session == 0xFFFF_FFFF {
            return String::new();
        }
        let user = wts_string(session, WTSUserName);
        let domain = wts_string(session, WTSDomainName);
        format_console_user(&domain, &user, &crate::hostname())
    }
}

#[cfg(windows)]
unsafe fn wts_string(session: u32, class: windows::Win32::System::RemoteDesktop::WTS_INFO_CLASS) -> String {
    use windows::core::PWSTR;
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTS_CURRENT_SERVER_HANDLE,
    };
    let mut buf = PWSTR::null();
    let mut bytes = 0u32;
    if WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), session, class, &mut buf, &mut bytes).is_err() {
        return String::new();
    }
    let text = if buf.is_null() { String::new() } else { buf.to_string().unwrap_or_default() };
    if !buf.is_null() {
        WTSFreeMemory(buf.as_ptr().cast());
    }
    text
}

#[cfg(not(windows))]
fn os_facts() -> (String, String) {
    ("linux".into(), String::new())
}

#[cfg(not(windows))]
fn live_console_user() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_user_drops_the_computer_name() {
        assert_eq!(format_console_user("BENLENOVODESK", "Ben", "BENLENOVODESK"), "Ben");
        assert_eq!(format_console_user("SPADER", "Ben", "BENLENOVODESK"), "SPADER\\Ben");
        assert_eq!(format_console_user("", "Ben", "BENLENOVODESK"), "Ben");
        assert_eq!(format_console_user("SPADER", "", "BENLENOVODESK"), "");
    }

    #[test]
    fn an_empty_console_keeps_the_last_user() {
        let dir = std::env::temp_dir().join(format!("pjdesk-console-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("console-user.txt");
        assert_eq!(remember_console_user(&path, "Ben"), "Ben");
        assert_eq!(remember_console_user(&path, ""), "Ben");
        assert_eq!(remember_console_user(&path, "Ada"), "Ada");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
