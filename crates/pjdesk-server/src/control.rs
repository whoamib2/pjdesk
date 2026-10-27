//! Commands the unelevated management window sends to the running service.
//!
//! The data folder is locked to SYSTEM and Administrators. After an update the
//! window comes back in the signed-in user's session, without elevation, so it
//! cannot write that folder itself. It asks the service over
//! `\\.\pipe\PJDeskServerControl`. The pipe is local. Its ACL allows SYSTEM,
//! Administrators, and interactive users to connect. The service then accepts
//! the command only when the caller is an administrator (including a UAC-filtered
//! token, checked through the linked token) or the request carries a valid
//! pjdesk.com access token for the account this server is signed in to.
//! Sign-in is the exception: the interactive window may sign in without elevation
//! and without an existing token, because that is how the token is created.
//! The password is checked by pjdesk.com. Every command is written to the audit
//! log. Passwords and tokens are not.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cloud::CloudConfig;

#[allow(dead_code)]
pub const PIPE_NAME: &str = r"\\.\pipe\PJDeskServerControl";

#[cfg_attr(not(windows), allow(dead_code))]
const MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Request {
    pub cmd: String,
    /// A pjdesk.com access token for the account this server is already signed in to.
    #[serde(default)]
    pub account_token: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub require_accounts: Option<bool>,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub auto: Option<bool>,
    #[serde(default)]
    pub window_start: String,
    #[serde(default)]
    pub window_end: String,
    #[serde(default)]
    pub watch_auto: Option<bool>,
    /// `join` or `replace`, for a sign-in that asked which copy to keep.
    #[serde(default)]
    pub choice: String,
    /// Handle returned when sign-in needs a choice. Not a secret.
    #[serde(default)]
    pub pending: String,
    /// Computer to block or unblock. A nine-digit PJDesk ID.
    #[serde(default)]
    pub device_id: String,
    /// Label for a new server access key. Not a secret.
    #[serde(default)]
    pub label: String,
    /// `0`, one day, 7 days, or 30 days.
    #[serde(default)]
    pub expires_secs: u64,
    #[serde(default)]
    pub computer_limit: u32,
    /// Access key id to revoke. Not a secret.
    #[serde(default)]
    pub key_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    /// `ask` when sign-in needs a choice before it can finish.
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub pending: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub written_at: u64,
    #[serde(default)]
    pub readable: bool,
    /// Set only when a key was just created. The caller shows it once.
    /// It is not written to the audit log.
    #[serde(default)]
    pub secret: String,
    /// Access keys after create or revoke, so the window can list them immediately.
    /// The secret and its hash are not in this list.
    #[serde(default)]
    pub keys: Vec<pjdesk_proto::signal::AccessKeyBrief>,
    /// Account email from `session` or a sign-in that kept the saved session.
    /// The token stays in the service. This field is not a secret.
    #[serde(default)]
    pub email: String,
}

fn ok(message: impl Into<String>) -> Response {
    Response { ok: true, message: message.into(), ..Response::default() }
}

fn fail(message: impl Into<String>) -> Response {
    Response { ok: false, message: message.into(), ..Response::default() }
}

/// True when this Windows token should be allowed to manage the server.
/// `linked_admin` is the full token behind a UAC filter.
pub fn windows_manager(elevated: bool, admin_on_token: bool, linked_admin: bool) -> bool {
    elevated || admin_on_token || linked_admin
}

struct HeldSignIn {
    cfg: CloudConfig,
    at: Instant,
}

fn pending_sign_in() -> &'static Mutex<Option<(String, HeldSignIn)>> {
    static PENDING: Mutex<Option<(String, HeldSignIn)>> = Mutex::new(None);
    &PENDING
}

fn same_secret(left: &str, right: &str) -> bool {
    let a = left.as_bytes();
    let b = right.as_bytes();
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The token belongs to the PJDesk account this server is signed in to.
pub fn account_token_ok(data_dir: &Path, token: &str) -> bool {
    let token = token.trim();
    if token.len() < 20 || token.len() > 128 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return false;
    }
    let Some(cfg) = CloudConfig::load(data_dir) else { return false };
    if !cfg.is_account() || cfg.email.trim().is_empty() {
        return false;
    }
    if same_secret(token, cfg.token.trim()) {
        return true;
    }
    let api = if cfg.api.trim().is_empty() {
        crate::cloud::default_api()
    } else {
        cfg.api.trim().trim_end_matches('/').to_string()
    };
    #[derive(Deserialize)]
    struct Me {
        #[serde(default)]
        email: String,
    }
    let response = crate::cloud::agent()
        .get(&format!("{api}/api/me"))
        .set("authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(8))
        .call();
    let Ok(response) = response else { return false };
    let Ok(text) = response.into_string() else { return false };
    let Ok(me) = serde_json::from_str::<Me>(&text) else { return false };
    me.email.eq_ignore_ascii_case(cfg.email.trim())
}

fn audit(data_dir: &Path, actor: &str, detail: &str) {
    crate::audit::AuditLog::open(data_dir).record("control", actor, detail);
    tracing::info!("control {actor}: {detail}");
}

/// Same actor and line, at most once a minute. The window probes with ping.
fn audit_throttled(data_dir: &Path, actor: &str, detail: &str) {
    let mut last = ping_denials().lock().unwrap();
    let now = Instant::now();
    if let Some((who, line, at)) = last.as_ref() {
        if who == actor && line == detail && now.duration_since(*at) < Duration::from_secs(60) {
            return;
        }
    }
    *last = Some((actor.to_string(), detail.to_string(), now));
    drop(last);
    audit(data_dir, actor, detail);
}

fn ping_denials() -> &'static Mutex<Option<(String, String, Instant)>> {
    static LAST: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);
    &LAST
}

/// Runs one command. The caller has already been authorized.
pub fn execute(data_dir: &Path, req: &Request, actor: &str) -> Response {
    let cmd = req.cmd.trim();
    let response = match cmd {
        "ping" => ok("ready"),
        "session" => session_status(data_dir),
        "sign_in" => sign_in(data_dir, req),
        "sign_in_choose" => sign_in_choose(data_dir, req),
        "sign_in_cancel" => sign_in_cancel(req),
        "sign_out" => {
            crate::cloud::sign_out(data_dir);
            ok("Signed out of the PJDesk account. The cloud copy is kept.")
        }
        "promote" => promote(data_dir),
        "demote" => demote(data_dir),
        "sync" => match crate::requests::push(data_dir, crate::requests::SYNC_FILE, "now") {
            Ok(()) => ok("Syncing now."),
            Err(e) => fail(format!("Could not ask the server to sync: {e}")),
        },
        "check" => match crate::self_update::check_now(data_dir) {
            Ok(msg) => ok(msg),
            Err(e) => fail(format!("{e:#}")),
        },
        "update_now" => match crate::self_update::install_if_planned(data_dir, true) {
            Ok(msg) => ok(msg),
            Err(e) => fail(format!("{e:#}")),
        },
        "set_update" => set_update(data_dir, req),
        "set_window" => match crate::self_update::set_window(data_dir, &req.window_start, &req.window_end) {
            Ok(()) => ok("Saved the maintenance window. Leave both blank for any time."),
            Err(e) => fail(format!("{e}")),
        },
        "set_require" => set_require(data_dir, req),
        "set_address" => set_address(data_dir, req),
        "set_watch" => set_watch(data_dir, req),
        "block" => block_computer(data_dir, &req.device_id),
        "unblock" => unblock_computer(data_dir, &req.device_id),
        "create_access_key" => create_access_key(data_dir, req),
        "revoke_access_key" => revoke_access_key_cmd(data_dir, &req.key_id),
        "restart" => schedule_restart(),
        _ => fail("That command was not understood."),
    };
    // A successful ping is the window's liveness check, about every few seconds.
    // Session is the same kind of read: the window asks it on a timer.
    // Denied pings and every other command are written to the audit log.
    let quiet = cmd == "ping" || cmd == "session";
    if !quiet || !response.ok {
        let outcome = if response.ok { "ok" } else { "denied" };
        audit(data_dir, actor, &format!("{cmd} {outcome}: {}", response.message));
    }
    response
}

fn same_saved_account(data_dir: &Path, email: &str) -> Option<String> {
    let saved = crate::cloud::saved_account_email(data_dir);
    if !saved.is_empty() && saved.eq_ignore_ascii_case(email.trim()) {
        Some(saved)
    } else {
        None
    }
}

fn session_status(data_dir: &Path) -> Response {
    let email = crate::cloud::saved_account_email(data_dir);
    let mut response = if email.is_empty() {
        ok("Not signed in.")
    } else {
        ok(format!("Signed in as {email}."))
    };
    response.email = email;
    response
}

fn sign_in(data_dir: &Path, req: &Request) -> Response {
    let email = req.email.trim().to_string();
    // A fresh window can ask again while the service already holds this account.
    // Keep that session. A new login would replace the token and can fail.
    if let Some(saved) = same_saved_account(data_dir, &email) {
        tracing::info!("sign-in: {email}: already signed in; the saved session stays");
        let mut response = ok(format!("Signed in as {saved}. Syncing as before."));
        response.email = saved;
        return response;
    }
    let password = req.password.clone();
    if email.is_empty() || password.is_empty() {
        tracing::info!("sign-in: not sent (email or password empty)");
        return fail("Enter your PJDesk account email and password.");
    }
    tracing::info!("sign-in: attempt for {email}");
    let cfg = match crate::cloud::sign_in(&crate::cloud::default_api(), &email, &password) {
        Ok(cfg) => cfg,
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::info!("sign-in: failed for {email}: {msg}");
            return fail(msg);
        }
    };
    let response = finish_sign_in(data_dir, cfg, None);
    if response.ok {
        tracing::info!("sign-in: ok for {email}: {}", response.message);
    } else {
        tracing::info!("sign-in: failed for {email}: {}", response.message);
    }
    response
}

fn sign_in_choose(data_dir: &Path, req: &Request) -> Response {
    let id = req.pending.trim();
    if id.is_empty() {
        return fail("That sign-in choice has expired. Sign in again.");
    }
    let choice = match req.choice.trim() {
        "join" => crate::cloud::Choice::Join,
        "replace" => crate::cloud::Choice::Replace,
        _ => return fail("Choose whether this server becomes the standby or replaces the cloud copy."),
    };
    let held = {
        let mut slot = pending_sign_in().lock().unwrap();
        match slot.take() {
            Some((stored, held)) if stored == id && held.at.elapsed() < Duration::from_secs(10 * 60) => held.cfg,
            _ => return fail("That sign-in choice has expired. Sign in again."),
        }
    };
    finish_sign_in(data_dir, held, Some(choice))
}

fn sign_in_cancel(req: &Request) -> Response {
    let mut slot = pending_sign_in().lock().unwrap();
    if let Some((stored, held)) = slot.take() {
        if stored == req.pending.trim() {
            crate::cloud::discard_sign_in(&held.cfg);
        }
    }
    ok("Sign-in cancelled.")
}

fn finish_sign_in(data_dir: &Path, cfg: CloudConfig, choice: Option<crate::cloud::Choice>) -> Response {
    let email = cfg.email.clone();
    let online = online_count(data_dir);
    match crate::cloud::connect_account(data_dir, &cfg, online, choice) {
        Ok(crate::cloud::Connected::Ask { from, written_at, readable }) => {
            let id = hex::encode(pjdesk_proto::random_bytes::<8>());
            *pending_sign_in().lock().unwrap() = Some((
                id.clone(),
                HeldSignIn { cfg, at: Instant::now() },
            ));
            Response {
                ok: true,
                message: format!("This account already holds {from}'s data. Choose which copy to keep."),
                code: "ask".into(),
                pending: id,
                from,
                written_at,
                readable,
                secret: String::new(),
                email: String::new(),
                ..Response::default()
            }
        }
        Ok(done) => {
            crate::cloud::publish_network(data_dir, online, false);
            let _ = crate::requests::push(data_dir, crate::requests::SYNC_FILE, "now");
            ok(match done {
                crate::cloud::Connected::Uploaded => format!("Signed in as {email}. This server's data is backed up."),
                crate::cloud::Connected::Resumed => format!("Signed in as {email}. Syncing as before."),
                crate::cloud::Connected::Joined { from } => {
                    format!("Signed in as {email}. This server now has {from}'s data and stays on standby.")
                }
                crate::cloud::Connected::Replaced => {
                    format!("Signed in as {email}. This server's data replaced the copy in PJDesk Cloud.")
                }
                crate::cloud::Connected::Ask { .. } => "Signed in.".into(),
            })
        }
        Err(e) => fail(format!("{e:#}")),
    }
}

fn promote(data_dir: &Path) -> Response {
    if crate::cloud::status_is_fresh(data_dir) {
        return match crate::requests::push(data_dir, crate::requests::PROMOTE_FILE, "now") {
            Ok(()) => ok("Asking the running server to become the primary."),
            Err(e) => fail(format!("Could not ask the server to become the primary: {e}")),
        };
    }
    match crate::cloud::promote(data_dir) {
        Ok(()) => ok("This server is now the primary."),
        Err(e) => fail(format!("{e:#}")),
    }
}

fn demote(data_dir: &Path) -> Response {
    if crate::cloud::status_is_fresh(data_dir) {
        return match crate::requests::push(data_dir, crate::requests::DEMOTE_FILE, "now") {
            Ok(()) => ok("Asking the running server to step down to standby."),
            Err(e) => fail(format!("Could not ask the server to step down: {e}")),
        };
    }
    match crate::cloud::demote(data_dir) {
        Ok(()) => ok("This server stepped down to standby."),
        Err(e) => fail(format!("{e:#}")),
    }
}

fn set_update(data_dir: &Path, req: &Request) -> Response {
    let Some(auto) = req.auto else {
        return fail("Missing the automatic-update setting.");
    };
    crate::self_update::set_auto(data_dir, auto);
    match crate::self_update::set_task(data_dir, auto) {
        Ok(()) => ok(if auto {
            "Automatic server updates are on. The service checks about once an hour."
        } else {
            "Automatic server updates are off."
        }),
        Err(e) => ok(format!("Saved the setting. {e:#}")),
    }
}

fn set_require(data_dir: &Path, req: &Request) -> Response {
    let Some(require) = req.require_accounts else {
        return fail("Missing the signed-in accounts setting.");
    };
    let mut policy = crate::access::Policy::load(data_dir);
    policy.require_accounts = require;
    policy.require_chosen = true;
    if let Err(e) = policy.save(data_dir) {
        return fail(format!("Could not save the signed-in accounts setting: {e}"));
    }
    if require {
        if let Err(e) = crate::requests::push(data_dir, crate::requests::REQUIRE_DISCONNECT_FILE, "now") {
            return fail(format!("Saved the setting, but could not ask the server to disconnect unsigned computers: {e}"));
        }
    }
    let dir = data_dir.to_path_buf();
    std::thread::spawn(move || crate::cloud::push_require_accounts(&dir, require));
    ok(if require {
        "Require signed-in accounts is on."
    } else {
        "Require signed-in accounts is off. Already-registered computers keep connecting."
    })
}

fn set_address(data_dir: &Path, req: &Request) -> Response {
    let address = req.address.trim().to_string();
    if address.is_empty() {
        return fail("Type the public address computers use to reach this server.");
    }
    let mut record = crate::public_addr::load(data_dir).unwrap_or(crate::public_addr::PublicAddress {
        address: String::new(),
        confirmed: false,
        source: String::new(),
        needs_confirm: false,
        note: String::new(),
    });
    if record.address.trim() != address {
        record.source = "manual".into();
    }
    record.address = address;
    record.confirmed = true;
    record.needs_confirm = false;
    record.note = "Publishing this address…".into();
    crate::public_addr::save(data_dir, &record);
    let signed_in = CloudConfig::load(data_dir).is_some_and(|cfg| cfg.is_account() && !cfg.token.is_empty());
    if signed_in {
        let dir = data_dir.to_path_buf();
        let online = online_count(data_dir);
        let replace = req.replace;
        std::thread::spawn(move || crate::cloud::publish_network(&dir, online, replace));
        ok("Publishing this address to My network.")
    } else {
        ok("Saved the address on this server. Sign in with your PJDesk account to publish it.")
    }
}

fn set_watch(data_dir: &Path, req: &Request) -> Response {
    let Some(auto) = req.watch_auto else {
        return fail("Missing the public IP setting.");
    };
    let mut watch = crate::public_addr::IpWatch::load(data_dir);
    watch.auto = Some(auto);
    watch.save(data_dir);
    ok(if auto {
        "This server will update the address when its public IP changes."
    } else {
        "This server will leave the address as you set it."
    })
}

fn schedule_restart() -> Response {
    #[cfg(windows)]
    {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return fail(format!("Cannot find the server program: {e}")),
        };
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new(exe)
                .args(["service", "restart"])
                .creation_flags(0x0800_0000)
                .status();
        });
        return ok("Restarting the PJDesk service.");
    }
    #[cfg(not(windows))]
    {
        fail("Restarting the service is only available on Windows.")
    }
}

fn block_computer(data_dir: &Path, id: &str) -> Response {
    match crate::requests::block_device(data_dir, id.trim()) {
        Ok(()) => ok(format!(
            "Removed {}. It is disconnected and cannot register again until it is unblocked.",
            pjdesk_proto::format_id(id.trim())
        )),
        Err(e) => fail(format!("Could not remove that computer: {e}")),
    }
}

fn create_access_key(data_dir: &Path, req: &Request) -> Response {
    match crate::access_keys::Store::create(data_dir, &req.label, req.expires_secs, req.computer_limit) {
        Ok(secret) => Response {
            ok: true,
            message: "Copy this key now. It is shown once.".into(),
            secret,
            keys: crate::access_keys::Store::public_list(data_dir),
            ..Response::default()
        },
        Err(e) => fail(e),
    }
}

fn revoke_access_key_cmd(data_dir: &Path, id: &str) -> Response {
    match crate::requests::revoke_access_key(data_dir, id.trim()) {
        Ok(()) => Response {
            ok: true,
            message: "Revoked. Every computer that used it will be disconnected and removed.".into(),
            keys: crate::access_keys::Store::public_list(data_dir),
            ..Response::default()
        },
        Err(e) => fail(format!("Could not revoke that key: {e}")),
    }
}

fn unblock_computer(data_dir: &Path, id: &str) -> Response {
    match crate::requests::unblock_device(data_dir, id.trim()) {
        Ok(()) => ok(format!("{} can register again.", pjdesk_proto::format_id(id.trim()))),
        Err(e) => fail(format!("Could not unblock that computer: {e}")),
    }
}

fn online_count(data_dir: &Path) -> usize {
    #[derive(Deserialize)]
    struct Row {
        id: String,
    }
    let path = data_dir.join("online.json");
    if !crate::cloud::online_file_is_fresh(&path) {
        return 0;
    }
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Row>>(&b).ok())
        .map(|rows| rows.into_iter().filter(|r| r.id.len() == 9).count())
        .unwrap_or(0)
}

/// Sign-in does not need an existing token. The interactive window uses it to
/// create one. Other commands stay limited to an administrator or that token.
pub fn interactive_sign_in(cmd: &str) -> bool {
    matches!(cmd.trim(), "sign_in" | "sign_in_choose" | "sign_in_cancel")
}

/// The window may ask whether the service is signed in. The answer is the
/// email only. The token stays in `cloud-sync.json` on the service.
pub fn interactive_session(cmd: &str) -> bool {
    cmd.trim() == "session"
}

/// Email in a "Signed in as …" line. The rest of the sentence is not part of it.
pub fn email_from_sign_in_message(message: &str) -> Option<String> {
    let rest = message.trim().strip_prefix("Signed in as ")?;
    let email = rest.split_whitespace().next()?.trim_end_matches('.');
    if email.contains('@') { Some(email.to_string()) } else { None }
}

/// What the account card shows. A locked folder does not mean signed out:
/// the service's answer is the account, and until that answer arrives the
/// last status-pipe email is kept so the form does not flash.
pub fn shown_account(readable: bool, local: &str, service_known: bool, service_email: &str, live: &str) -> String {
    if readable {
        return local.to_string();
    }
    if service_known {
        return service_email.to_string();
    }
    live.to_string()
}

/// The sign-in form is for a service that answered and has no account, or a
/// window that can read the folder and found none. It stays hidden while the
/// service has not answered yet.
pub fn show_sign_in_form(readable: bool, email: &str, service_known: bool) -> bool {
    email.is_empty() && (readable || service_known)
}

/// Applies a request after the pipe has checked the caller.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn execute_authorized(data_dir: &Path, req: &Request, windows_ok: bool, actor: &str) -> Response {
    let cmd = req.cmd.trim();
    if cmd == "ping" && !windows_ok && req.account_token.trim().is_empty() {
        audit_throttled(data_dir, actor, "ping denied: caller is not an administrator");
        return fail("Not allowed.");
    }
    let account_ok = !windows_ok && account_token_ok(data_dir, &req.account_token);
    if !windows_ok && !account_ok && !interactive_sign_in(cmd) && !interactive_session(cmd) {
        audit(data_dir, actor, &format!("{cmd} denied: caller is not an administrator"));
        return fail(
            "This Windows user cannot manage the server. An administrator can, and so can the pjdesk.com account this server is signed in to.",
        );
    }
    let who = if windows_ok { actor.to_string() } else { format!("account:{actor}") };
    // execute records the command and the result. It does not write the password or the token.
    execute(data_dir, req, &who)
}

#[cfg(windows)]
pub fn serve(data_dir: std::path::PathBuf) {
    loop {
        if !serve_once(&data_dir) {
            std::thread::sleep(Duration::from_millis(400));
        }
    }
}

#[cfg(not(windows))]
pub fn serve(_data_dir: std::path::PathBuf) {}

#[cfg(windows)]
fn serve_once(data_dir: &Path) -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SY and BA full. IU (interactive) can connect. Remote clients are rejected below.
        let sddl = w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)");
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None).is_err() {
            return false;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let pipe = CreateNamedPipeW(
            w!(r"\\.\pipe\PJDeskServerControl"),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            4,
            64 * 1024,
            16 * 1024,
            0,
            Some(&sa),
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if pipe == INVALID_HANDLE_VALUE {
            return false;
        }
        let connected = match ConnectNamedPipe(pipe, None) {
            Ok(()) => true,
            Err(e) => (e.code().0 as u32 & 0xFFFF) == 535,
        };
        if !connected {
            let _ = windows::Win32::Foundation::CloseHandle(pipe);
            return false;
        }
        let mut buf = vec![0u8; MAX_BYTES + 1];
        let mut n = 0u32;
        let read = ReadFile(pipe, Some(&mut buf), Some(&mut n), None);
        let response = if read.is_err() || n == 0 || n as usize > MAX_BYTES {
            fail("Empty request.")
        } else {
            match serde_json::from_slice::<Request>(&buf[..n as usize]) {
                Ok(req) => {
                    let (windows_ok, actor) = inspect_caller(pipe);
                    execute_authorized(data_dir, &req, windows_ok, &actor)
                }
                Err(_) => fail("That command was not understood."),
            }
        };
        if let Ok(bytes) = serde_json::to_vec(&response) {
            let _ = WriteFile(pipe, Some(&bytes), None, None);
        }
        let _ = DisconnectNamedPipe(pipe);
        let _ = windows::Win32::Foundation::CloseHandle(pipe);
        true
    }
}

#[cfg(windows)]
fn inspect_caller(pipe: windows::Win32::Foundation::HANDLE) -> (bool, String) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::RevertToSelf;
    use windows::Win32::System::Pipes::ImpersonateNamedPipeClient;
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

    unsafe {
        if ImpersonateNamedPipeClient(pipe).is_err() {
            return (false, "unknown".into());
        }
        let mut token = HANDLE::default();
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token);
        let (allowed, label) = if opened.is_ok() {
            let allowed = caller_token_is_manager(token);
            let label = token_label(token);
            let _ = CloseHandle(token);
            (allowed, label)
        } else {
            (false, "unknown".into())
        };
        let _ = RevertToSelf();
        (allowed, label)
    }
}

#[cfg(windows)]
fn caller_token_is_manager(token: windows::Win32::Foundation::HANDLE) -> bool {
    let elevated = token_is_elevated(token);
    let admin = token_in_admins(token);
    let linked = linked_token_in_admins(token);
    windows_manager(elevated, admin, linked)
}

#[cfg(windows)]
fn token_is_elevated(token: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION};
    unsafe {
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        ok.is_ok() && elev.TokenIsElevated != 0
    }
}

#[cfg(windows)]
fn token_in_admins(token: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows::Win32::Security::{CheckTokenMembership, PSID};
    unsafe {
        let mut sid = PSID::default();
        if ConvertStringSidToSidW(w!("S-1-5-32-544"), &mut sid).is_err() {
            return false;
        }
        let mut member = windows::core::BOOL(0);
        let ok = CheckTokenMembership(Some(token), sid, &mut member);
        let _ = LocalFree(Some(HLOCAL(sid.0)));
        ok.is_ok() && member.as_bool()
    }
}

#[cfg(windows)]
fn linked_token_in_admins(token: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateToken, GetTokenInformation, SecurityImpersonation, TokenLinkedToken, TOKEN_LINKED_TOKEN,
    };
    unsafe {
        let mut linked = TOKEN_LINKED_TOKEN { LinkedToken: HANDLE::default() };
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenLinkedToken,
            Some(&mut linked as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_LINKED_TOKEN>() as u32,
            &mut ret,
        );
        if ok.is_err() || linked.LinkedToken.is_invalid() {
            return false;
        }
        let mut imp = HANDLE::default();
        let duplicated = DuplicateToken(linked.LinkedToken, SecurityImpersonation, &mut imp);
        let allowed = if duplicated.is_ok() {
            let yes = token_is_elevated(imp) || token_in_admins(imp);
            let _ = CloseHandle(imp);
            yes
        } else {
            token_is_elevated(linked.LinkedToken)
        };
        let _ = CloseHandle(linked.LinkedToken);
        allowed
    }
}

#[cfg(windows)]
fn token_label(token: windows::Win32::Foundation::HANDLE) -> String {
        use windows::Win32::Foundation::LocalFree;
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_USER};
    unsafe {
        let mut need = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut need);
        if need == 0 || need > 4096 {
            return "windows-user".into();
        }
        let mut buf = vec![0u64; need.div_ceil(8) as usize];
        let mut ret = 0u32;
        if GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), need, &mut ret).is_err() {
            return "windows-user".into();
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut raw = windows::core::PWSTR::null();
        if ConvertSidToStringSidW(user.User.Sid, &mut raw).is_err() || raw.is_null() {
            return "windows-user".into();
        }
        let sid = raw.to_string().unwrap_or_else(|_| "windows-user".into());
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(raw.as_ptr() as *mut _)));
        sid
    }
}

/// How long the window waits for the service. A stuck pipe must not leave Remove spinning.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// Sign-in derives the auth key (PBKDF2) and then talks to pjdesk.com.
/// That is longer than a local file change, so it does not use [`CALL_TIMEOUT`].
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(45);

/// Runs `job` and returns its response. If it is still going when `timeout` elapses,
/// returns a failed response with `code` `timeout`. `None` from `job` is preserved
/// (the service is not listening).
pub(crate) fn finish_within(
    timeout: Duration,
    job: impl FnOnce() -> Option<Response> + Send + 'static,
) -> Option<Response> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    match rx.recv_timeout(timeout) {
        Ok(value) => value,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Some(Response {
            ok: false,
            code: "timeout".into(),
            message: "The PJDesk service did not answer.".into(),
            ..Response::default()
        }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

/// One round trip. `None` when the service is not listening.
/// A response with `code` `timeout` means the pipe was asked and did not finish.
pub fn call(req: &Request) -> Option<Response> {
    call_for(req, CALL_TIMEOUT)
}

/// Same as [`call`], with the caller's deadline. Sign-in uses [`SIGN_IN_WAIT`].
pub fn call_for(req: &Request, timeout: Duration) -> Option<Response> {
    #[cfg(windows)]
    {
        let req = req.clone();
        return finish_within(timeout, move || call_pipe(&req));
    }
    #[cfg(not(windows))]
    {
        let _ = (req, timeout);
        None
    }
}

/// One round trip. `None` when the service is not listening.
#[cfg(windows)]
fn call_pipe(req: &Request) -> Option<Response> {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let bytes = serde_json::to_vec(req).ok()?;
    if bytes.len() > MAX_BYTES {
        return Some(fail("That request is too large."));
    }
    unsafe {
        if !WaitNamedPipeW(w!(r"\\.\pipe\PJDeskServerControl"), 1500).as_bool() {
            return None;
        }
        let handle = CreateFileW(
            w!(r"\\.\pipe\PJDeskServerControl"),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()?;
        let wrote = WriteFile(handle, Some(&bytes), None, None);
        if wrote.is_err() {
            let _ = CloseHandle(handle);
            return None;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let mut n = 0u32;
            if ReadFile(handle, Some(&mut chunk), Some(&mut n), None).is_err() || n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
            if buf.len() > MAX_BYTES {
                break;
            }
        }
        let _ = CloseHandle(handle);
        serde_json::from_slice(&buf).ok()
    }
}

/// Ping with the signed-in account token, so the owner is allowed without elevation.
/// An empty token still succeeds for an administrator, including a filtered UAC token.
pub fn ready_with(account_token: &str) -> bool {
    call(&Request {
        cmd: "ping".into(),
        account_token: account_token.to_string(),
        ..Request::default()
    })
    .is_some_and(|r| r.ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pjdesk-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_filtered_admin_token_is_still_a_manager() {
        assert!(windows_manager(false, false, true));
        assert!(windows_manager(true, false, false));
        assert!(windows_manager(false, true, false));
        assert!(!windows_manager(false, false, false));
    }

    #[test]
    fn settings_commands_write_the_data_folder_and_the_log_has_no_password() {
        let dir = dir("settings");
        let req = Request {
            cmd: "set_require".into(),
            require_accounts: Some(false),
            password: "super-secret-password".into(),
            ..Request::default()
        };
        let response = execute(&dir, &req, "S-1-5-32-544");
        assert!(response.ok, "{}", response.message);
        let policy = crate::access::Policy::load(&dir);
        assert!(!policy.require_accounts);
        assert!(policy.require_chosen);
        let turned_on = execute(
            &dir,
            &Request { cmd: "set_require".into(), require_accounts: Some(true), ..Request::default() },
            "S-1-5-32-544",
        );
        assert!(turned_on.ok);
        assert!(crate::access::Policy::load(&dir).require_accounts);

        let address = execute(
            &dir,
            &Request { cmd: "set_address".into(), address: "pjdesk.spader.us:21200".into(), ..Request::default() },
            "S-1-5-32-544",
        );
        assert!(address.ok, "{}", address.message);
        assert_eq!(
            crate::public_addr::load(&dir).unwrap().address,
            "pjdesk.spader.us:21200"
        );

        let watch = execute(
            &dir,
            &Request { cmd: "set_watch".into(), watch_auto: Some(false), ..Request::default() },
            "S-1-5-32-544",
        );
        assert!(watch.ok);
        assert_eq!(crate::public_addr::IpWatch::load(&dir).auto, Some(false));

        let update = execute(
            &dir,
            &Request { cmd: "set_update".into(), auto: Some(false), ..Request::default() },
            "S-1-5-32-544",
        );
        assert!(update.ok, "{}", update.message);
        assert!(!crate::self_update::Settings::load(&dir).auto);

        let unknown = execute(&dir, &Request { cmd: "format".into(), ..Request::default() }, "S-1-5-32-544");
        assert!(!unknown.ok);

        let log = std::fs::read_to_string(dir.join("audit.log")).unwrap_or_default();
        assert!(log.contains("set_require"));
        assert!(!log.contains("super-secret-password"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stored_account_token_authorizes_and_a_short_one_does_not() {
        let dir = dir("token");
        let token = "abcdefghijklmnopqrstuvwxyz012345";
        let cfg = serde_json::json!({
            "mode": "account",
            "api": "http://127.0.0.1:9",
            "email": "ben@spader.us",
            "token": token,
            "key": "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
        });
        std::fs::write(dir.join("cloud-sync.json"), cfg.to_string()).unwrap();
        assert!(account_token_ok(&dir, token));
        assert!(!account_token_ok(&dir, "short"));
        assert!(!account_token_ok(&dir, "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
        let denied = execute_authorized(
            &dir,
            &Request { cmd: "ping".into(), ..Request::default() },
            false,
            "S-1-5-32-545",
        );
        assert!(!denied.ok);
        let allowed = execute_authorized(
            &dir,
            &Request { cmd: "ping".into(), account_token: token.into(), ..Request::default() },
            false,
            "S-1-5-32-545",
        );
        assert!(allowed.ok, "{}", allowed.message);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unelevated_sign_in_reaches_the_password_check() {
        let dir = dir("signin-open");
        let response = execute_authorized(
            &dir,
            &Request {
                cmd: "sign_in".into(),
                email: "ben@spader.us".into(),
                password: String::new(),
                ..Request::default()
            },
            false,
            "S-1-5-32-545",
        );
        assert!(!response.ok);
        assert_eq!(response.message, "Enter your PJDesk account email and password.");
        let blocked = execute_authorized(
            &dir,
            &Request {
                cmd: "set_require".into(),
                require_accounts: Some(true),
                password: "secret-pass".into(),
                ..Request::default()
            },
            false,
            "S-1-5-32-545",
        );
        assert!(!blocked.ok);
        assert!(blocked.message.contains("cannot manage"), "{}", blocked.message);
        assert!(!crate::access::Policy::load(&dir).require_accounts);
        let log = std::fs::read_to_string(dir.join("audit.log")).unwrap_or_default();
        assert!(log.contains("sign_in"), "{log}");
        assert!(!log.contains("secret-pass"), "{log}");
        assert!(interactive_sign_in("sign_in"));
        assert!(interactive_sign_in("sign_in_choose"));
        assert!(!interactive_sign_in("set_require"));
        assert!(interactive_session("session"));
        assert!(!interactive_session("sign_in"));
        assert_eq!(email_from_sign_in_message("Signed in as ben@spader.us. Syncing as before."), Some("ben@spader.us".into()));
        assert!(email_from_sign_in_message("Enter your email and password.").is_none());
        assert_eq!(shown_account(false, "", true, "ben@spader.us", ""), "ben@spader.us");
        assert_eq!(shown_account(false, "", false, "", "ben@spader.us"), "ben@spader.us");
        assert_eq!(shown_account(false, "", true, "", "ben@spader.us"), "");
        assert!(!show_sign_in_form(false, "", false));
        assert!(show_sign_in_form(false, "", true));
        assert!(!show_sign_in_form(false, "ben@spader.us", true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_window_reads_the_service_account_and_a_second_sign_in_keeps_it() {
        let session_dir = dir("session");
        let cfg = r#"{"mode":"account","email":"ben@spader.us","token":"tok_abcdefghijklmnopqrstuvwxyz","key":"abcd"}"#;
        std::fs::write(session_dir.join("cloud-sync.json"), cfg).unwrap();
        let response = execute(&session_dir, &Request { cmd: "session".into(), ..Request::default() }, "window");
        assert!(response.ok, "{}", response.message);
        assert_eq!(response.email, "ben@spader.us");
        assert_eq!(std::fs::read_to_string(session_dir.join("cloud-sync.json")).unwrap(), cfg);
        let again = execute(
            &session_dir,
            &Request {
                cmd: "sign_in".into(),
                email: "Ben@Spader.us".into(),
                password: "not-used".into(),
                ..Request::default()
            },
            "window",
        );
        assert!(again.ok, "{}", again.message);
        assert!(again.message.contains("Syncing as before"), "{}", again.message);
        assert_eq!(again.email, "ben@spader.us");
        let kept = std::fs::read_to_string(session_dir.join("cloud-sync.json")).unwrap();
        assert!(kept.contains("tok_abcdefghijklmnopqrstuvwxyz"), "{kept}");
        assert!(session_dir.join("cloud-sync.json").exists());
        let open = execute_authorized(
            &session_dir,
            &Request { cmd: "session".into(), ..Request::default() },
            false,
            "S-1-5-32-545",
        );
        assert!(open.ok, "{}", open.message);
        assert_eq!(open.email, "ben@spader.us");
        let empty = dir("session-empty");
        let none = execute(&empty, &Request { cmd: "session".into(), ..Request::default() }, "window");
        assert!(none.ok, "{}", none.message);
        assert!(none.email.is_empty());
        assert!(!empty.join("cloud-sync.json").exists());
        let _ = std::fs::remove_dir_all(&session_dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn other_local_users_are_denied_before_a_command_runs() {
        let dir = dir("deny");
        std::fs::write(dir.join("network-access.json"), b"{\"require_accounts\":false}").unwrap();
        let response = execute_authorized(
            &dir,
            &Request { cmd: "set_require".into(), require_accounts: Some(true), ..Request::default() },
            false,
            "S-1-5-32-545",
        );
        assert!(!response.ok);
        assert!(!crate::access::Policy::load(&dir).require_accounts);
        let log = std::fs::read_to_string(dir.join("audit.log")).unwrap_or_default();
        assert!(log.contains("denied"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slow_pipe_returns_a_timeout_instead_of_waiting_forever() {
        let started = Instant::now();
        let response = finish_within(Duration::from_millis(40), || {
            std::thread::sleep(Duration::from_millis(250));
            Some(ok("too late"))
        })
        .expect("timeout is a response");
        assert!(!response.ok);
        assert_eq!(response.code, "timeout");
        assert!(response.message.contains("did not answer"));
        assert!(started.elapsed() < Duration::from_secs(1));
        let ready = finish_within(Duration::from_secs(1), || Some(ok("ready"))).unwrap();
        assert!(ready.ok);
        assert!(finish_within(Duration::from_secs(1), || None).is_none());
    }

    #[test]
    fn an_offline_computer_is_removed_without_a_live_connection() {
        let dir = dir("offline-block");
        let mut reg = crate::registry::Registry::load(dir.join("devices.json")).unwrap();
        let secret = [3u8; 32];
        let id = reg.register(None, &secret, "DESKTOP-D7E4AOR", "0.6.33").unwrap();
        reg.note_address(&id, "203.0.113.8");
        let response = execute(
            &dir,
            &Request { cmd: "block".into(), device_id: id.clone(), ..Request::default() },
            "account:ben@spader.us",
        );
        assert!(response.ok, "{}", response.message);
        assert!(!response.message.is_empty());
        let reg = crate::registry::Registry::load(dir.join("devices.json")).unwrap();
        assert!(reg.list().iter().all(|row| row.id != id));
        assert_eq!(reg.blocked().len(), 1);
        assert_eq!(reg.blocked()[0].id, id);
        assert_eq!(reg.blocked()[0].last_ip, "203.0.113.8");
        assert_eq!(
            crate::requests::take(&dir, crate::requests::BLOCK_FILE),
            vec![id]
        );
        let missing = execute(
            &dir,
            &Request { cmd: "block".into(), device_id: "111222333".into(), ..Request::default() },
            "account:ben@spader.us",
        );
        assert!(!missing.ok);
        assert!(missing.message.contains("not registered"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_require_on_asks_for_unsigned_computers_to_be_disconnected() {
        let dir = dir("require-drop");
        let response = execute(
            &dir,
            &Request { cmd: "set_require".into(), require_accounts: Some(true), ..Request::default() },
            "account:ben@spader.us",
        );
        assert!(response.ok, "{}", response.message);
        assert!(crate::access::Policy::load(&dir).require_accounts);
        assert_eq!(
            crate::requests::take(&dir, crate::requests::REQUIRE_DISCONNECT_FILE),
            vec!["now".to_string()]
        );
        let off = execute(
            &dir,
            &Request { cmd: "set_require".into(), require_accounts: Some(false), ..Request::default() },
            "account:ben@spader.us",
        );
        assert!(off.ok, "{}", off.message);
        assert!(crate::requests::take(&dir, crate::requests::REQUIRE_DISCONNECT_FILE).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
