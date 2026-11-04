//! Windows half of [`super::inputroute`]: the dedicated input thread, the
//! client side of the SYSTEM input agent, and the agent itself.

use std::io::{Read, Write};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::Foundation::{GetLastError, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::inputroute::{self, CheckOutcome, DeskFollower, Rec, SelfCheck};

struct Req {
    recs: Vec<Rec>,
    /// Pixel the cursor should end up on (absolute moves only).
    want: Option<(i32, i32)>,
    reply: SyncSender<bool>,
}

static QUEUE: OnceLock<std::sync::Mutex<SyncSender<Req>>> = OnceLock::new();

fn to_rec(i: &INPUT) -> Rec {
    unsafe {
        if i.r#type == INPUT_KEYBOARD {
            let k = i.Anonymous.ki;
            Rec { kind: inputroute::REC_KEYBOARD, a: k.wVk.0 as i32, b: k.wScan as i32, c: k.dwFlags.0, d: 0 }
        } else {
            let m = i.Anonymous.mi;
            Rec { kind: inputroute::REC_MOUSE, a: m.dx, b: m.dy, c: m.mouseData, d: m.dwFlags.0 }
        }
    }
}

fn from_rec(r: &Rec) -> INPUT {
    if r.kind == inputroute::REC_KEYBOARD {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(r.a as u16),
                    wScan: r.b as u16,
                    dwFlags: KEYBD_EVENT_FLAGS(r.c),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    } else {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: r.a,
                    dy: r.b,
                    mouseData: r.c,
                    dwFlags: MOUSE_EVENT_FLAGS(r.d),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
}

/// Sends `inputs` on the input thread; true if Windows accepted all of them.
/// `want` is the pixel an absolute move should land on (for the self-check).
/// Ctrl+Alt+Del: through the SYSTEM input agent when it is running, else
/// directly (only works when this process is SYSTEM). True when sent.
pub fn send_sas() -> bool {
    if let Some(session) = crate::platform::session_id() {
        let name = inputroute::agent_pipe_name(session);
        if let Ok(mut pipe) = std::fs::OpenOptions::new().read(true).write(true).open(&name) {
            let mut reply = [0u8; 5];
            if pipe.write_all(&inputroute::encode(&inputroute::sas_request())).is_ok() && pipe.read_exact(&mut reply).is_ok() {
                return reply[0] == 1;
            }
            tracing::warn!("Ctrl+Alt+Del: the input agent did not answer (older agent?)");
        }
    }
    crate::platform::send_ctrl_alt_del()
}

pub fn submit(inputs: &[INPUT], want: Option<(i32, i32)>) -> bool {
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = sync_channel::<Req>(256);
        std::thread::Builder::new()
            .name("pjdesk-input".into())
            .spawn(move || input_thread(rx))
            .expect("spawn input thread");
        std::sync::Mutex::new(tx)
    });
    let (reply, wait) = sync_channel(1);
    let req = Req { recs: inputs.iter().map(to_rec).collect(), want, reply };
    let Ok(tx) = queue.lock().map(|q| q.clone()) else { return false };
    if tx.send(req).is_err() {
        return false;
    }
    wait.recv().unwrap_or(false)
}

/// SendInput on the current thread, following the input desktop: attach when
/// due, and after a rejection re-attach and retry once (so a button-up or
/// key-up sent across a desktop switch is not lost).
fn send_following(desk: &mut DeskFollower, inputs: &[INPUT]) -> (bool, u32) {
    let now = Instant::now();
    if desk.should_attach(now, crate::platform::input_desktop_name) {
        attach(desk, now);
    }
    let size = std::mem::size_of::<INPUT>() as i32;
    if unsafe { SendInput(inputs, size) } as usize == inputs.len() {
        return (true, 0);
    }
    let first = unsafe { GetLastError() }.0;
    desk.invalidate();
    attach(desk, Instant::now());
    if unsafe { SendInput(inputs, size) } as usize == inputs.len() {
        tracing::info!("Input recovered after re-attaching to desktop \"{}\" (was error {first})", desk.current());
        return (true, 0);
    }
    (false, unsafe { GetLastError() }.0)
}

fn attach(desk: &mut DeskFollower, now: Instant) {
    let before = desk.current().to_string();
    let ok = crate::platform::attach_thread_to_input_desktop();
    let name = crate::platform::input_desktop_name();
    if before != name.clone().unwrap_or_default() || !ok {
        tracing::info!(
            "Input thread {} input desktop \"{}\"",
            if ok { "attached to" } else { "could not attach to" },
            name.as_deref().unwrap_or("?")
        );
    }
    desk.attached(now, name);
}

struct Agent {
    pipe: std::fs::File,
}

impl Agent {
    fn connect() -> Option<Agent> {
        let session = crate::platform::session_id()?;
        let name = inputroute::agent_pipe_name(session);
        let pipe = std::fs::OpenOptions::new().read(true).write(true).open(&name).ok()?;
        tracing::info!("Input goes through the PJDesk SYSTEM input agent (session {session})");
        Some(Agent { pipe })
    }

    fn send(&mut self, recs: &[Rec]) -> std::io::Result<(bool, u32)> {
        self.pipe.write_all(&inputroute::encode(recs))?;
        let mut reply = [0u8; 5];
        self.pipe.read_exact(&mut reply)?;
        Ok((reply[0] == 1, u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]])))
    }
}

fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

fn input_thread(rx: Receiver<Req>) {
    let mut desk = DeskFollower::default();
    let mut agent: Option<Agent> = None;
    let mut agent_tried: Option<Instant> = None;
    let mut check = SelfCheck::default();
    let mut failures_logged = 0u32;
    while let Ok(req) = rx.recv() {
        if agent.is_none() && agent_tried.is_none_or(|t| t.elapsed() >= inputroute::AGENT_RETRY) {
            agent_tried = Some(Instant::now());
            agent = Agent::connect();
        }
        let mut result = None;
        if let Some(a) = agent.as_mut() {
            match a.send(&req.recs) {
                Ok(r) => result = Some(r),
                Err(e) => {
                    tracing::warn!("Input agent went away ({e}); injecting locally until it is back");
                    agent = None;
                    agent_tried = Some(Instant::now());
                }
            }
        }
        let (ok, err) = result.unwrap_or_else(|| {
            let inputs: Vec<INPUT> = req.recs.iter().map(from_rec).collect();
            send_following(&mut desk, &inputs)
        });
        if !ok && failures_logged < 20 {
            failures_logged += 1;
            tracing::warn!("Windows rejected injected input (error {err}) on desktop \"{}\"", desk.current());
        }
        let _ = req.reply.send(ok);
        if let (true, Some(want)) = (ok, req.want) {
            // Keep this thread on the input desktop so GetCursorPos reads the
            // real cursor, then judge whether the move landed.
            let now = Instant::now();
            if desk.should_attach(now, crate::platform::input_desktop_name) {
                attach(&mut desk, now);
            }
            match check.observe(want, cursor_pos()) {
                CheckOutcome::Fine => {}
                CheckOutcome::Recover => {
                    tracing::warn!("Mouse moves are not landing; re-attaching input and reconnecting the agent");
                    desk.invalidate();
                    agent = None;
                    agent_tried = None;
                }
                CheckOutcome::Report => {
                    let msg = if agent.is_some() {
                        "Windows is not accepting PJDesk's mouse input right now. PJDesk keeps retrying."
                    } else {
                        "The window in front on the remote computer is running as administrator, so Windows blocks input from PJDesk. Install the PJDesk service on that computer so input can reach admin windows."
                    };
                    tracing::warn!("{msg}");
                    if let Ok(mut p) = inputroute::INPUT_PROBLEM.lock() {
                        *p = Some(msg.to_string());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- agent

/// `pjdesk __input-agent`: started by the service as SYSTEM in the active
/// console session. Injects for the PJDesk host in the same session, so input
/// reaches elevated windows and the secure desktop.
pub fn run_agent() -> anyhow::Result<()> {
    let session = crate::platform::session_id().ok_or_else(|| anyhow::anyhow!("no session id"))?;
    tracing::info!("PJDesk input agent started in session {session}");
    loop {
        match listen_once(session) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!("Input agent pipe: {e:#}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
}

/// SDDL: SYSTEM, plus the user signed in to `session` (read/write only).
fn pipe_sddl(session: u32) -> String {
    match session_user_sid(session) {
        Some(sid) => format!("D:P(A;;GA;;;SY)(A;;GRGW;;;{sid})"),
        None => "D:P(A;;GA;;;SY)".to_string(),
    }
}

fn session_user_sid(session: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_USER};
    use windows::Win32::System::RemoteDesktop::WTSQueryUserToken;
    unsafe {
        let mut token = HANDLE::default();
        WTSQueryUserToken(session, &mut token).ok()?;
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as _), buf.len() as u32, &mut len);
        let _ = CloseHandle(token);
        ok.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        out
    }
}

fn listen_once(session: u32) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };
    let name = windows::core::HSTRING::from(inputroute::agent_pipe_name(session));
    let sddl = windows::core::HSTRING::from(pipe_sddl(session));
    let pipe = unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let pipe = CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            8,
            4096,
            4096,
            0,
            Some(&sa),
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if pipe == INVALID_HANDLE_VALUE {
            anyhow::bail!("CreateNamedPipe failed: {}", GetLastError().0);
        }
        if let Err(e) = ConnectNamedPipe(pipe, None) {
            if (e.code().0 as u32 & 0xFFFF) != 535 {
                let _ = CloseHandle(pipe);
                anyhow::bail!("ConnectNamedPipe: {e}");
            }
        }
        pipe
    };
    let raw = pipe.0 as isize;
    std::thread::spawn(move || {
        let pipe = HANDLE(raw as *mut core::ffi::c_void);
        if client_is_pjdesk(pipe, session) {
            serve_client(pipe);
        } else {
            tracing::warn!("Input agent refused a client that is not PJDesk in this session");
        }
        unsafe {
            let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    });
    Ok(())
}

/// Only the PJDesk executable (same path as this one, which only admins can
/// write) in the same session may drive the agent.
fn client_is_pjdesk(pipe: windows::Win32::Foundation::HANDLE, session: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let mut pid = 0u32;
        if GetNamedPipeClientProcessId(pipe, &mut pid).is_err() {
            return false;
        }
        let mut sid = u32::MAX;
        if ProcessIdToSessionId(pid, &mut sid).is_err() || sid != session {
            return false;
        }
        let Ok(proc_) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc_, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(proc_);
        if ok.is_err() {
            return false;
        }
        let client = String::from_utf16_lossy(&buf[..len as usize]);
        let Ok(me) = std::env::current_exe() else { return false };
        client.eq_ignore_ascii_case(&me.to_string_lossy())
    }
}

fn serve_client(pipe: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    let read_exact = |buf: &mut [u8]| -> bool {
        let mut at = 0;
        while at < buf.len() {
            let mut n = 0u32;
            if unsafe { ReadFile(pipe, Some(&mut buf[at..]), Some(&mut n), None) }.is_err() || n == 0 {
                return false;
            }
            at += n as usize;
        }
        true
    };
    let mut desk = DeskFollower::default();
    loop {
        let mut count = [0u8; 1];
        if !read_exact(&mut count) {
            return;
        }
        let Some(len) = inputroute::body_len(count[0]) else { return };
        let mut body = vec![0u8; len];
        if !read_exact(&mut body) {
            return;
        }
        let Some(recs) = inputroute::decode(&body) else { return };
        if inputroute::is_sas(&recs) {
            let ok = crate::platform::send_ctrl_alt_del();
            if unsafe { WriteFile(pipe, Some(&inputroute::encode_reply(ok, 0)), None, None) }.is_err() {
                return;
            }
            continue;
        }
        let inputs: Vec<INPUT> = recs.iter().map(from_rec).collect();
        let (ok, err) = send_following(&mut desk, &inputs);
        let reply = inputroute::encode_reply(ok, err);
        if unsafe { WriteFile(pipe, Some(&reply), None, None) }.is_err() {
            return;
        }
    }
}
