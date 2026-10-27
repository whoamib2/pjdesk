//! One session window can hold several computers.
//!
//! The first connect starts the viewer process. That process listens on
//! 127.0.0.1 and writes `port` and its pid next to the client config. Later
//! connects from this Windows sign-in go there and become tabs, instead of
//! opening another window. The password is only on this localhost socket.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A computer the open session window should add as a tab.
///
/// `password` is required. An empty password still starts its own process so
/// the main window can ask after the other computer answers.
#[derive(Clone, Serialize, Deserialize)]
pub struct Admit {
    pub id: String,
    pub password: String,
    pub server: String,
    pub direct: String,
    pub label: String,
    pub files: bool,
    pub remember: bool,
    pub from_store: bool,
    pub force_relay: bool,
    pub view: u8,
    pub display: u8,
}

impl std::fmt::Debug for Admit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admit")
            .field("id", &self.id)
            .field("server", &self.server)
            .field("direct", &self.direct)
            .field("label", &self.label)
            .field("files", &self.files)
            .finish()
    }
}

/// Closing the window asks first when more than one session is open.
pub fn window_close_needs_confirm(open: usize) -> bool {
    open > 1
}

/// Which remaining session to show after `closing` goes away.
///
/// Prefers the tab to the right, then the one to the left. `None` means that
/// was the last live session and the window should close.
pub fn focus_after_close(order: &[u64], closing: u64, live: &[u64]) -> Option<u64> {
    let pos = order.iter().position(|id| *id == closing).unwrap_or(0);
    order
        .iter()
        .skip(pos + 1)
        .copied()
        .find(|id| live.contains(id))
        .or_else(|| order.iter().take(pos).rev().copied().find(|id| live.contains(id)))
}

pub fn listener_path() -> PathBuf {
    crate::config::Config::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("session-window.txt")
}

fn result_path() -> PathBuf {
    crate::config::Config::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("session-admit.txt")
}

pub fn publish_listener(port: u16) -> std::io::Result<()> {
    let path = listener_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{port}\n{}\n", std::process::id()))
}

pub fn read_listener() -> Option<(u16, u32)> {
    read_listener_at(&listener_path())
}

pub fn read_listener_at(path: &Path) -> Option<(u16, u32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let pid = lines.next()?.trim().parse().ok()?;
    Some((port, pid))
}

/// Removes the listener file when it still names this process and `port`.
pub fn take_down_listener(port: u16) {
    let path = listener_path();
    if read_listener_at(&path).is_some_and(|(p, pid)| p == port && pid == std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

/// Tells the main window that an in-window connect failed the way a child exit would.
pub fn write_admit_result(id: &str, code: i32, files: bool) {
    let path = result_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{code}\n{id}\n{}\n", if files { 1 } else { 0 }));
}

pub fn take_admit_result() -> Option<(String, i32, bool)> {
    let path = result_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let mut lines = text.lines();
    let code = lines.next()?.trim().parse().ok()?;
    let id = lines.next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let files = lines.next()?.trim() == "1";
    Some((id, code, files))
}

const MAX_ADMIT: usize = 256 * 1024;

pub fn encode(admit: &Admit) -> Result<Vec<u8>, postcard::Error> {
    let body = postcard::to_allocvec(admit)?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend(body);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Option<Admit> {
    if bytes.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
    if len > MAX_ADMIT || bytes.len() != 4 + len {
        return None;
    }
    postcard::from_bytes(&bytes[4..]).ok()
}

pub fn read_admit(stream: &mut impl Read) -> Option<Admit> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).ok()?;
    let len = u32::from_be_bytes(head) as usize;
    if len > MAX_ADMIT {
        return None;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    postcard::from_bytes(&body).ok()
}

/// Sends one computer to the session window. True when that window accepted it.
pub fn send_admit(port: u16, admit: &Admit) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(400)) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
    let Ok(bytes) = encode(admit) else { return false };
    if stream.write_all(&bytes).is_err() {
        return false;
    }
    let mut buf = [0u8; 8];
    let Ok(n) = stream.read(&mut buf) else { return false };
    buf.get(..n).is_some_and(|reply| reply.starts_with(b"ok"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Admit {
        Admit {
            id: "123456789".into(),
            password: "s e c r e t".into(),
            server: "pjdesk.example:21116".into(),
            direct: String::new(),
            label: "Cabin Desk".into(),
            files: false,
            remember: true,
            from_store: false,
            force_relay: false,
            view: 0,
            display: 1,
        }
    }

    #[test]
    fn closing_the_window_asks_only_when_several_sessions_are_open() {
        assert!(!window_close_needs_confirm(0));
        assert!(!window_close_needs_confirm(1));
        assert!(window_close_needs_confirm(2));
    }

    #[test]
    fn closing_a_tab_focuses_the_session_beside_it() {
        let order = [1u64, 2, 3];
        let live = [1u64, 2, 3];
        assert_eq!(focus_after_close(&order, 2, &live), Some(3));
        assert_eq!(focus_after_close(&order, 3, &live), Some(2));
        assert_eq!(focus_after_close(&order, 1, &[1]), None);
    }

    #[test]
    fn admit_keeps_the_password_and_debug_does_not() {
        let admit = sample();
        let bytes = encode(&admit).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.password, "s e c r e t");
        assert_eq!(back.label, "Cabin Desk");
        assert_eq!(back.display, 1);
        let shown = format!("{admit:?}");
        assert!(!shown.contains("s e c r e t"));
        assert!(shown.contains("Cabin Desk"));
    }

    #[test]
    fn listener_file_is_port_and_pid() {
        let path = std::env::temp_dir().join(format!("pjdesk-session-window-{}-test.txt", std::process::id()));
        std::fs::write(&path, "21234\n4242\n").unwrap();
        assert_eq!(read_listener_at(&path), Some((21234, 4242)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn admit_round_trip_over_localhost() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let admit = sample();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let got = read_admit(&mut sock).unwrap();
            sock.write_all(b"ok\n").unwrap();
            got
        });
        assert!(send_admit(port, &admit));
        let got = server.join().unwrap();
        assert_eq!(got.id, admit.id);
        assert_eq!(got.password, admit.password);
        assert_eq!(got.server, admit.server);
    }
}
