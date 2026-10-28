//! Messages on the QUIC connection between controller and host.
//!
//! Streams:
//! * **Control** (bidirectional, opened by the controller): the SPAKE2
//!   handshake ([`AuthMsg`]), then [`ControlMsg`] from controller to host
//!   (input, frame acks, keyframe requests) and [`HostMsg`] back.
//! * **Video** (unidirectional, opened by the host after authentication):
//!   a sequence of H.264 access units, each `[u16 header len][VideoHeader]
//!   [u32 data len][Annex-B bitstream]`.
//!
//! Latency control: the host keeps at most a few frames "in flight" (sent but
//! not yet acknowledged by the controller's decoder) and simply captures less
//! often when the network or the decoder is slow, so frames never queue up.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest encoded frame accepted (a 4K keyframe is far below this).
pub const MAX_VIDEO_FRAME: usize = 16 * 1024 * 1024;
/// Largest video header accepted. A header is a few dozen bytes; anything
/// larger is a peer trying to make the host or controller allocate.
pub const MAX_VIDEO_HEADER: usize = 4096;

/// Password-authenticated key exchange (see [`crate::auth`]).
///
/// The host may accept more than one password at once (a persistent permanent
/// password and/or a rotating one-time password). The controller therefore
/// sends one SPAKE2 message per "slot" with the single password it is trying,
/// and the host replies with one message + confirmation per password it offers;
/// the controller then names the slot whose confirmation verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMsg {
    /// Controller → host: one SPAKE2 message A per candidate slot (same password).
    Start { proto: u16, spakes: Vec<Vec<u8>>, controller_name: String },
    /// Host → controller: a SPAKE2 message B + key confirmation for each
    /// password the host accepts (index-aligned with the controller's slots).
    Challenge { spakes: Vec<Vec<u8>>, confirms: Vec<[u8; 32]> },
    /// Controller → host: which slot matched (`0xFF` = none) and its confirmation.
    Confirm { slot: u8, confirm: [u8; 32] },
    /// Host → controller: final verdict.
    Result { ok: bool, message: String, host_name: String },
    /// Controller → host: skip the password and ask the person at the computer.
    /// Appended after [`AuthMsg::Result`]. An older host cannot decode it.
    AskPerson { proto: u16, controller_name: String },
    /// Controller → host: ask to view the screen, without a password.
    /// `account` is the pjdesk.com account when the controller is signed in.
    /// Otherwise it is empty and the host uses `computer`, then `id`.
    /// Appended after [`AuthMsg::AskPerson`]. An older host cannot decode it.
    AskView { proto: u16, account: String, computer: String, id: String },
    /// 0.6.69+. Controller → host, on its own stream before any password:
    /// does this computer have a permanent password? An older host cannot
    /// decode it; the controller then treats the answer as unknown.
    Probe { proto: u16 },
    /// 0.6.69+. Host → controller answer to [`AuthMsg::Probe`]. No secret:
    /// only whether a permanent password is set.
    Facts { permanent: bool },
}

/// Controller → host messages after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMsg {
    Input(InputEvent),
    /// The controller decoded frame `seq`; the host may send another.
    FrameAck {
        seq: u32,
    },
    /// The decoder lost sync; please send an IDR frame.
    RequestKeyframe,
    /// A special command the host performs itself (e.g. Ctrl+Alt+Del).
    Special(SpecialCommand),
    Ping {
        t_us: u64,
    },
    /// Version 3. Announces what the controller wants. The host answers with
    /// [`HostMsg::Welcome`]. A version-2 host never receives this.
    Hello {
        permissions: u32,
        monitor: u8,
        quality: crate::quality::StreamQuality,
    },
    Chat {
        text: String,
    },
    Clipboard(Clipboard),
    SetQuality(crate::quality::StreamQuality),
    SelectMonitor {
        index: u8,
    },
    File(FileMsg),
    /// Ask Windows to restart. Requires [`crate::perms::RESTART`].
    Restart,
    Privacy {
        enabled: bool,
    },
    BlockInput {
        enabled: bool,
    },
    SetKeyboardMode(KeyboardMode),
    Tunnel(TunnelMsg),
    /// Unicode input used when the keyboard mode is [`KeyboardMode::Translate`].
    Text {
        chars: String,
    },
    /// Start or stop host system-audio. Samples follow on a side stream.
    Audio {
        enabled: bool,
    },
    /// 0.5.2. Watch one display, every display, or one window per display.
    /// Sent only after the host has published [`HostMsg::Displays`]. An older
    /// host never receives this variant. `mode` is [`VIEW_SINGLE`], [`VIEW_ALL`],
    /// or [`VIEW_SEPARATE`].
    View {
        mode: u8,
        display: u8,
    },
    /// 0.5.2. Displays that still have a local window. The host captures only
    /// these. Sent only after [`HostMsg::Displays`].
    Watch {
        displays: Vec<u8>,
    },
    /// 0.5.2. Pointer inside one display, normalized the same way as
    /// [`InputEvent::MouseMove`]. The host maps it onto that display's
    /// virtual-desktop rectangle, which may begin at a negative coordinate.
    Pointer {
        display: u8,
        x: u16,
        y: u16,
    },
    /// 0.6.8. Who is at the controller. Sent only after [`HostMsg::AskVisitor`].
    /// An older host never sends that, so it never receives this variant.
    Visitor {
        account: String,
        computer: String,
        user: String,
        id: String,
    },
    /// 0.6.50. One chat line. Sent only after the host has sent
    /// [`HostMsg::ChatReady`]. An older host never sends that, so it never
    /// receives this variant. `unix` is seconds since the Unix epoch.
    /// `name` is the controller's account, or its computer name when there is
    /// no account.
    ChatLine {
        text: String,
        name: String,
        unix: u64,
    },
}

/// [`ControlMsg::View`] mode: one display, the one named by `display`.
pub const VIEW_SINGLE: u8 = 0;
/// [`ControlMsg::View`] mode: every display, stitched at its real position.
pub const VIEW_ALL: u8 = 1;
/// [`ControlMsg::View`] mode: one local window per display. The open set is
/// [`ControlMsg::Watch`].
pub const VIEW_SEPARATE: u8 = 2;

/// Commands the controller asks the host to carry out directly.
///
/// New commands are added at the end. A version-2 host only understands
/// [`SpecialCommand::CtrlAltDel`]; newer commands are sent only to a version-3 host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialCommand {
    /// Secure attention sequence (Ctrl+Alt+Del). Works when the host runs as
    /// the background service; a normal app can't inject it.
    CtrlAltDel,
    /// Lock the host session (the Windows lock screen).
    Lock,
    /// Open Task Manager on the host.
    TaskManager,
    /// Android global action: Back. Sent only to a host that reported
    /// `family = "android"`.
    Back,
    /// Android global action: Home.
    Home,
    /// Android global action: Recents (overview).
    Recents,
    /// Raise the media volume on an Android host.
    VolumeUp,
    /// Lower the media volume on an Android host.
    VolumeDown,
    /// Android power dialog, or the lock screen when the device offers it.
    Power,
    /// Open the notification shade. Sent only to an Android host.
    Notifications,
}

/// How keystrokes are delivered. Map sends hardware scan codes (the layout of
/// the focused app on the host). Translate sends Unicode characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyboardMode {
    Map,
    Translate,
}

/// One display the host can share. Kept as it was in 0.4.0 so a Welcome from
/// this build still decodes on a 0.4/0.5.1 controller. Position and DPI ride
/// on [`DisplayLayout`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub index: u8,
    pub width: u16,
    pub height: u16,
    pub primary: bool,
    pub name: String,
}

/// One display, including where it sits on the virtual desktop.
///
/// `x` and `y` are negative when the display is left of or above the primary.
/// `dpi` is the effective DPI, or 0 when the host could not read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayLayout {
    pub index: u8,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    pub dpi: u32,
}

/// Where one display sits inside the separate-windows picture.
///
/// Coordinates are capture pixels, before the encoder scales the picture.
/// A display with no tile is not being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasTile {
    pub index: u8,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Clipboard contents. Text rides in `bytes` (UTF-8). Images are PNG bytes and,
/// when larger than a control frame, travel on a side stream with `bytes` empty
/// and `byte_len` set. A view-only session never sends or accepts these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clipboard {
    pub kind: ClipKind,
    #[serde(default)]
    pub bytes: Vec<u8>,
    /// Total payload size. Equals `bytes.len()` when the bytes are inline.
    pub byte_len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Text,
    ImagePng,
}

/// File-transfer control. The bytes themselves use a separate QUIC stream so
/// they never block keyboard input. See [`crate::filesafe`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileMsg {
    Offer {
        id: u64,
        name: String,
        size: u64,
        resume_from: u64,
    },
    Accept {
        id: u64,
        resume_from: u64,
    },
    Reject {
        id: u64,
        reason: String,
    },
    Progress {
        id: u64,
        bytes: u64,
    },
    Done {
        id: u64,
    },
    Cancel {
        id: u64,
    },
    Error {
        id: u64,
        reason: String,
    },
    /// Ask the host to list files in its receive folder. `path` must be empty;
    /// the host does not browse the rest of the disk.
    List {
        id: u64,
        path: String,
    },
    /// Names in that folder. `path` echoes the request.
    Entries {
        id: u64,
        path: String,
        names: Vec<String>,
    },
    /// Ask the host to send one file from its receive folder.
    Pull {
        id: u64,
        name: String,
    },
}

/// A TCP forward. Data uses its own bidirectional QUIC stream, headed by the id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelMsg {
    Open { id: u64, remote_host: String, remote_port: u16 },
    Ready { id: u64 },
    Close { id: u64, reason: String },
}

/// First bytes of a side stream (file, audio, clipboard image).
pub const FILE_MAGIC: &[u8; 8] = b"PJFILE1\0";
pub const AUDIO_MAGIC: &[u8; 8] = b"PJAUDI1\0";
pub const CLIP_MAGIC: &[u8; 8] = b"PJCLIP1\0";

/// Largest chunk on a side stream. Kept well under the control-frame cap.
pub const SIDE_CHUNK: usize = 64 * 1024;
/// Largest clipboard image accepted (PNG).
pub const MAX_CLIP_IMAGE: u32 = 4 * 1024 * 1024;
/// Largest inline clipboard text. Larger text is refused rather than split.
pub const MAX_CLIP_TEXT: usize = 48 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideHeader {
    pub id: u64,
    pub offset: u64,
    /// For audio: sample rate. For clipboard: width. Otherwise unused.
    pub a: u32,
    /// For audio: channels. For clipboard: height. Otherwise unused.
    pub b: u32,
}

/// Host → controller messages after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    Pong {
        t_us: u64,
    },
    /// Human-readable status for the controller's title bar / console.
    Notice(String),
    /// Version 3. Permissions actually granted, and the monitors the host can share.
    Welcome {
        permissions: u32,
        monitors: Vec<MonitorInfo>,
        proto: u16,
    },
    Chat {
        text: String,
    },
    Clipboard(Clipboard),
    File(FileMsg),
    /// The host accepted a restart and Windows is about to reboot.
    Restarting,
    Privacy {
        enabled: bool,
    },
    Tunnel(TunnelMsg),
    /// A request was refused; the session continues.
    Denied {
        what: String,
        reason: String,
    },
    /// 0.5.2. Every attached display. Sent only when Hello carried
    /// [`crate::perms::LAYOUT`], and again when a display is added, removed,
    /// or resized. An older controller never sets that bit, so it never
    /// receives this variant.
    Displays {
        displays: Vec<DisplayLayout>,
    },
    /// 0.5.2. Host operating system. Sent once, with the same gate as
    /// [`HostMsg::Displays`]. `family` is `windows`, `macos`, `linux`, or
    /// `android`.
    Platform {
        family: String,
        version: String,
    },
    /// 0.5.2. Layout of the separate-windows picture. Sent only to a controller
    /// that asked for [`crate::perms::LAYOUT`], and only while that mode is on.
    Atlas {
        width: u16,
        height: u16,
        tiles: Vec<AtlasTile>,
    },
    /// 0.6.8. The host can show who connected. Sent only when Hello carried
    /// [`crate::perms::VISITOR`]. An older controller never sets that bit.
    AskVisitor,
    /// 0.6.47. The relay is moving to this primary. The controller reconnects
    /// there with the password it already has. Sent only by a 0.6.47 host
    /// after the server's [`crate::signal::ServerMsg::MoveRelay`].
    MoveRelay {
        primary: String,
    },
    /// 0.6.50. This host can do named session chat. Sent once, only when Hello
    /// carried [`crate::perms::CHAT_PEER`]. An older controller never sets that
    /// bit, so it never receives this variant.
    ChatReady,
    /// 0.6.50. One chat line from the person at the host. Sent only after
    /// [`HostMsg::ChatReady`]. `name` is the host computer name.
    ChatLine {
        text: String,
        name: String,
        unix: u64,
    },
    /// 0.6.65. This computer's PJDesk version. Sent only when Hello carried
    /// [`crate::perms::PEER_VERSION`]. An older controller never sets that bit,
    /// so it never receives this variant.
    PeerVersion {
        version: String,
    },
}

/// Whether the session toolbar may offer chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatGate {
    /// The peer's PJDesk version is known and new enough.
    Ready,
    /// The peer's PJDesk version is known and older than 0.6.50.
    TooOld,
    /// The handshake did not report a version and there is no fresh presence.
    /// That is not the same as an old peer.
    Unknown,
}

/// Handshake version wins. A fresh presence version is used only when the
/// handshake did not report a PJDesk version. A saved-list version that is not
/// fresh is ignored. `10` and `11` are operating-system versions, not PJDesk.
pub fn chat_gate(handshake_version: &str, presence_version: &str, presence_fresh: bool) -> ChatGate {
    match live_peer_version(handshake_version, presence_version, presence_fresh) {
        Some(version) if chat_supported(&version) => ChatGate::Ready,
        Some(_) => ChatGate::TooOld,
        None => ChatGate::Unknown,
    }
}

/// The PJDesk version to trust for a feature check.
pub fn live_peer_version(handshake_version: &str, presence_version: &str, presence_fresh: bool) -> Option<String> {
    if let Some(version) = pjdesk_app_version(handshake_version) {
        return Some(version);
    }
    if presence_fresh {
        if let Some(version) = pjdesk_app_version(presence_version) {
            return Some(version);
        }
    }
    None
}

/// A PJDesk release is `0.x`. Windows 10 and 11 are not.
fn pjdesk_app_version(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches('v');
    let (major, _, _) = crate::update::parse_version(raw)?;
    if major != 0 {
        return None;
    }
    Some(raw.to_string())
}

/// First PJDesk release that speaks named in-session chat.
pub const CHAT_MIN_VERSION: &str = "0.6.50";

/// Shown on a Chat button when the other computer is older than 0.6.50.
pub const CHAT_NEEDS_TIP: &str = "The other computer needs PJDesk 0.6.50 or newer";

/// Largest chat message accepted, in characters.
pub const MAX_CHAT_TEXT: usize = 2000;
/// Largest sender name stored on a chat line, in characters.
pub const MAX_CHAT_NAME: usize = 80;

/// One line kept for the current session. Nothing here is written to disk
/// as the session log, and nothing is sent to pjdesk.com.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatEntry {
    pub mine: bool,
    pub name: String,
    pub text: String,
    pub unix: u64,
}

/// True when `version` is a final release at least 0.6.50.
pub fn chat_supported(version: &str) -> bool {
    crate::update::version_at_least(version, CHAT_MIN_VERSION)
}

/// Seconds since the Unix epoch, or 0 when the clock is unset.
pub fn chat_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Plain text for a chat line. Empty and whitespace-only text is refused.
/// Control characters other than tab and newline are dropped. The result is
/// at most [`MAX_CHAT_TEXT`] characters.
pub fn chat_line(text: &str) -> Option<String> {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= MAX_CHAT_TEXT {
            break;
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() { None } else { Some(out) }
}

/// A sender name. Blank names use `fallback`, then "Computer".
pub fn chat_name(name: &str, fallback: &str) -> String {
    let cleaned = clean_name(name);
    if !cleaned.is_empty() {
        return cleaned;
    }
    let fallback = clean_name(fallback);
    if fallback.is_empty() { "Computer".into() } else { fallback }
}

fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_control() {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= MAX_CHAT_NAME {
            break;
        }
    }
    out.trim().to_string()
}

/// `HH:MM:SS UTC  name  text`. The clock is UTC from `unix` so it does not
/// depend on the computer's time zone.
pub fn format_chat_line(unix: u64, name: &str, text: &str) -> String {
    let secs = unix % 86_400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02} UTC  {name}  {text}")
}

/// Input from the controller, replayed on the host with `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Absolute pointer position, normalized to the shared display:
    /// 0 = left/top edge, 65535 = right/bottom edge.
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Wheel movement in Windows units (120 = one notch).
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// A physical key, as a PC/AT "set 1" scan code. Extended keys carry the
    /// 0xE0 prefix in the high byte (e.g. Left arrow = 0xE04B); Pause is 0xE11D.
    /// Numpad keys typed with the controller's NumLock on carry
    /// [`KEY_NUMPAD_DIGIT`], so they produce digits whatever the host's NumLock.
    Key {
        scan: u16,
        down: bool,
    },
    /// Release every key and button the controller may still be holding
    /// (sent when the viewer window loses focus).
    ReleaseAll,
    /// An accessibility gesture for an Android host.
    ///
    /// Coordinates are normalized to the shared picture, the same space as
    /// [`InputEvent::MouseMove`]. Sent only after the host has reported
    /// `family = "android"`. A Windows host ignores it.
    ///
    /// `kind` is [`crate::gesture::GESTURE_TAP`], [`crate::gesture::GESTURE_LONG_PRESS`],
    /// [`crate::gesture::GESTURE_SWIPE`], or [`crate::gesture::GESTURE_PINCH`].
    /// Points that a kind does not use are zero.
    Gesture {
        kind: u8,
        duration_ms: u16,
        x: [u16; 4],
        y: [u16; 4],
    },
}

/// Flag on [`InputEvent::Key`] scan codes: a numpad key meant as a digit.
pub const KEY_NUMPAD_DIGIT: u16 = 0x8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Metadata for one encoded video frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoHeader {
    pub seq: u32,
    pub keyframe: bool,
    /// Encoded picture size (may be smaller than the host screen if downscaled).
    pub width: u16,
    pub height: u16,
    /// Time spent converting + encoding on the host, in microseconds.
    pub encode_us: u32,
}

/// Writes one encoded frame to the video stream.
pub async fn write_video_frame<W>(w: &mut W, header: &VideoHeader, data: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let head = postcard::to_stdvec(header)?;
    let mut prefix = Vec::with_capacity(2 + head.len() + 4);
    prefix.extend_from_slice(&(head.len() as u16).to_be_bytes());
    prefix.extend_from_slice(&head);
    prefix.extend_from_slice(&(data.len() as u32).to_be_bytes());
    w.write_all(&prefix).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

/// Reads one encoded frame into `data` (reusing its allocation).
pub async fn read_video_frame<R>(r: &mut R, data: &mut Vec<u8>) -> Result<VideoHeader>
where
    R: AsyncRead + Unpin,
{
    let mut len2 = [0u8; 2];
    r.read_exact(&mut len2).await?;
    let head_len = u16::from_be_bytes(len2) as usize;
    if head_len > MAX_VIDEO_HEADER {
        bail!("video header too large ({head_len} bytes)");
    }
    let mut head = vec![0u8; head_len];
    r.read_exact(&mut head).await?;
    let header: VideoHeader = postcard::from_bytes(&head)?;
    let mut len4 = [0u8; 4];
    r.read_exact(&mut len4).await?;
    let len = u32::from_be_bytes(len4) as usize;
    if len > MAX_VIDEO_FRAME {
        bail!("video frame too large ({len} bytes)");
    }
    data.resize(len, 0);
    r.read_exact(data).await?;
    Ok(header)
}

/// Lines for the "someone connected" notice.
///
/// The headline is the pjdesk.com account when the controller is signed in.
/// Otherwise it is the controller's computer name, then its ID. The computer
/// name is a second line only when the account is the headline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitorCard {
    pub headline: String,
    pub secondary: String,
    pub detail: String,
}

fn visitor_label(text: &str, max: usize) -> String {
    text.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

pub fn visitor_card(account: &str, computer: &str, user: &str, id: &str) -> VisitorCard {
    let account = visitor_label(account, 80);
    let computer = visitor_label(computer, 64);
    let user = visitor_label(user, 80);
    let id_raw = visitor_label(id, 32);
    let id_shown = if crate::is_valid_id(&id_raw) { crate::format_id(&id_raw) } else { id_raw.clone() };
    let headline = if !account.is_empty() {
        account.clone()
    } else if !computer.is_empty() {
        computer.clone()
    } else {
        id_shown.clone()
    };
    let secondary = if !account.is_empty() && !computer.is_empty() && !computer.eq_ignore_ascii_case(&headline) {
        computer
    } else {
        String::new()
    };
    let mut parts = Vec::new();
    if !user.is_empty() {
        parts.push(user);
    }
    if !id_shown.is_empty() && !id_shown.eq_ignore_ascii_case(&headline) && !id_raw.eq_ignore_ascii_case(&headline) {
        parts.push(id_shown);
    }
    VisitorCard { headline, secondary, detail: parts.join(" · ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_person_is_appended_after_the_auth_result() {
        let result = postcard::to_stdvec(&AuthMsg::Result { ok: false, message: "wrong password".into(), host_name: String::new() }).unwrap();
        assert_eq!(result[0], 3);
        let ask = postcard::to_stdvec(&AuthMsg::AskPerson { proto: 3, controller_name: "Ben".into() }).unwrap();
        assert_eq!(ask[0], 4, "AskPerson is appended after Result");
        let view = postcard::to_stdvec(&AuthMsg::AskView {
            proto: 3,
            account: "ben@spader.us".into(),
            computer: "BENLENOVODESK".into(),
            id: "692996394".into(),
        })
        .unwrap();
        assert_eq!(view[0], 5, "AskView is appended after AskPerson");
    }

    #[tokio::test]
    async fn video_frame_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let header = VideoHeader { seq: 9, keyframe: true, width: 1920, height: 1080, encode_us: 1234 };
        let payload: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        write_video_frame(&mut a, &header, &payload).await.unwrap();
        let mut got = Vec::new();
        let h = read_video_frame(&mut b, &mut got).await.unwrap();
        assert_eq!(h.seq, 9);
        assert!(h.keyframe);
        assert_eq!((h.width, h.height), (1920, 1080));
        assert_eq!(got, payload);
    }

    #[tokio::test]
    async fn rejects_oversized_video_header() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&((MAX_VIDEO_HEADER as u16) + 1).to_be_bytes()).await.unwrap();
        let mut got = Vec::new();
        assert!(read_video_frame(&mut b, &mut got).await.is_err());
    }

    #[test]
    fn the_connect_notice_names_the_account_then_the_computer_then_the_id() {
        let signed = visitor_card("ben@spader.us", "BENLENOVODESK", "Ben", "692996394");
        assert_eq!(signed.headline, "ben@spader.us");
        assert_eq!(signed.secondary, "BENLENOVODESK");
        assert!(signed.detail.contains("Ben"));
        assert!(signed.detail.contains("692 996 394"));
        let named = visitor_card("Ben Spader", "BENLENOVODESK", "", "692996394");
        assert_eq!(named.headline, "Ben Spader");
        assert_eq!(named.secondary, "BENLENOVODESK");
        let local = visitor_card("", "BENLENOVODESK", "Ben", "692996394");
        assert_eq!(local.headline, "BENLENOVODESK");
        assert!(local.secondary.is_empty());
        let only_id = visitor_card("  ", "", "", "692996394");
        assert_eq!(only_id.headline, "692 996 394");
        assert!(only_id.detail.is_empty());
    }

    #[test]
    fn existing_variants_keep_their_indexes() {
        // Postcard encodes the variant index first. These indexes are the
        // version-2 contract. New variants are appended after them.
        assert_eq!(postcard::to_stdvec(&ControlMsg::Ping { t_us: 1 }).unwrap()[0], 4);
        assert_eq!(postcard::to_stdvec(&HostMsg::Notice("ok".into())).unwrap()[0], 1);
        assert_eq!(postcard::to_stdvec(&SpecialCommand::CtrlAltDel).unwrap()[0], 0);
        assert_eq!(postcard::to_stdvec(&HostMsg::Denied { what: "a".into(), reason: "b".into() }).unwrap()[0], 9);
        assert_eq!(postcard::to_stdvec(&ControlMsg::Audio { enabled: true }).unwrap()[0], 17);
        assert_eq!(postcard::to_stdvec(&ControlMsg::View { mode: VIEW_SINGLE, display: 0 }).unwrap()[0], 18);
        assert_eq!(postcard::to_stdvec(&HostMsg::Displays { displays: vec![] }).unwrap()[0], 10);
        assert_eq!(
            postcard::to_stdvec(&HostMsg::Platform { family: "linux".into(), version: String::new() }).unwrap()[0],
            11
        );
        assert_eq!(postcard::to_stdvec(&ControlMsg::Watch { displays: vec![0, 2] }).unwrap()[0], 19);
        assert_eq!(postcard::to_stdvec(&ControlMsg::Pointer { display: 1, x: 0, y: 0 }).unwrap()[0], 20);
        assert_eq!(postcard::to_stdvec(&HostMsg::Atlas { width: 1, height: 1, tiles: vec![] }).unwrap()[0], 12);
        assert_eq!(postcard::to_stdvec(&HostMsg::AskVisitor).unwrap()[0], 13);
        assert_eq!(
            postcard::to_stdvec(&ControlMsg::Visitor {
                account: String::new(),
                computer: String::new(),
                user: String::new(),
                id: String::new(),
            })
            .unwrap()[0],
            21
        );
        assert_eq!(postcard::to_stdvec(&InputEvent::ReleaseAll).unwrap()[0], 4);
        assert_eq!(
            postcard::to_stdvec(&InputEvent::Gesture { kind: 0, duration_ms: 40, x: [0; 4], y: [0; 4] }).unwrap()[0],
            5
        );
        assert_eq!(postcard::to_stdvec(&SpecialCommand::TaskManager).unwrap()[0], 2);
        assert_eq!(postcard::to_stdvec(&SpecialCommand::Back).unwrap()[0], 3);
        assert_eq!(postcard::to_stdvec(&SpecialCommand::Power).unwrap()[0], 8);
        assert_eq!(postcard::to_stdvec(&SpecialCommand::Notifications).unwrap()[0], 9);
        let hello = ControlMsg::Hello { permissions: 1, monitor: 0, quality: crate::quality::StreamQuality::default() };
        assert!(postcard::to_stdvec(&hello).unwrap()[0] > 4);
    }

    #[test]
    fn version3_messages_roundtrip() {
        let msgs = vec![
            ControlMsg::Chat { text: "hello".into() },
            ControlMsg::File(FileMsg::Offer { id: 7, name: "a.txt".into(), size: 10, resume_from: 0 }),
            ControlMsg::Clipboard(Clipboard { kind: ClipKind::Text, bytes: b"hi".to_vec(), byte_len: 2 }),
            ControlMsg::SelectMonitor { index: 1 },
            ControlMsg::Restart,
            ControlMsg::Privacy { enabled: true },
            ControlMsg::SetKeyboardMode(KeyboardMode::Translate),
            ControlMsg::Text { chars: "é".into() },
            ControlMsg::Tunnel(TunnelMsg::Open { id: 1, remote_host: "127.0.0.1".into(), remote_port: 22 }),
        ];
        for msg in msgs {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            let back: ControlMsg = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(format!("{msg:?}"), format!("{back:?}"));
        }
        let welcome = HostMsg::Welcome {
            permissions: crate::perms::CLASSIC,
            monitors: vec![MonitorInfo { index: 0, width: 1920, height: 1080, primary: true, name: "Primary".into() }],
            proto: 3,
        };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes(&bytes).unwrap(), HostMsg::Welcome { proto: 3, .. }));
    }

    #[test]
    fn chat_lines_are_appended_and_the_old_chat_index_stays() {
        assert_eq!(postcard::to_stdvec(&ControlMsg::Chat { text: "hello".into() }).unwrap()[0], 6);
        assert_eq!(postcard::to_stdvec(&HostMsg::Chat { text: "hello".into() }).unwrap()[0], 3);
        assert_eq!(postcard::to_stdvec(&HostMsg::MoveRelay { primary: "a".into() }).unwrap()[0], 14);
        let controller = ControlMsg::ChatLine { text: "hi".into(), name: "ben@spader.us".into(), unix: 1700000000 };
        let host = HostMsg::ChatLine { text: "hi".into(), name: "OFFICE".into(), unix: 1700000000 };
        assert_eq!(postcard::to_stdvec(&controller).unwrap()[0], 22);
        assert_eq!(postcard::to_stdvec(&HostMsg::ChatReady).unwrap()[0], 15);
        assert_eq!(postcard::to_stdvec(&host).unwrap()[0], 16);
        let back: ControlMsg = postcard::from_bytes(&postcard::to_stdvec(&controller).unwrap()).unwrap();
        assert!(matches!(back, ControlMsg::ChatLine { unix: 1700000000, .. }));
        let back: HostMsg = postcard::from_bytes(&postcard::to_stdvec(&host).unwrap()).unwrap();
        assert!(matches!(back, HostMsg::ChatLine { .. }));
        assert_eq!(CHAT_NEEDS_TIP, "The other computer needs PJDesk 0.6.50 or newer");
        assert!(!chat_supported("0.6.49"));
        assert!(chat_supported("0.6.50"));
        assert!(chat_supported("0.6.51"));
        assert!(!chat_supported("0.6.50-rc1"));
        assert_eq!(chat_gate("0.6.64", "0.6.43", true), ChatGate::Ready, "the handshake wins over presence");
        assert_eq!(chat_gate("0.6.49", "0.6.64", true), ChatGate::TooOld, "an old handshake is not replaced by presence");
        assert_eq!(chat_gate("", "0.6.64", true), ChatGate::Ready);
        assert_eq!(chat_gate("", "0.6.43", true), ChatGate::TooOld);
        assert_eq!(chat_gate("", "0.6.64", false), ChatGate::Unknown, "a saved-list version is not fresh presence");
        assert_eq!(chat_gate("", "", false), ChatGate::Unknown);
        assert_eq!(chat_gate("11", "0.6.64", true), ChatGate::Ready, "an operating-system version is not PJDesk");
        assert_eq!(chat_gate("10.0.22631", "", false), ChatGate::Unknown);
        assert_eq!(postcard::to_stdvec(&HostMsg::PeerVersion { version: "0.6.65".into() }).unwrap()[0], 17);
        assert!(chat_line("   ").is_none());
        assert!(chat_line("").is_none());
        assert_eq!(chat_line("  hello\u{0001} there  ").as_deref(), Some("hello there"));
        assert_eq!(format_chat_line(1700000000, "OFFICE", "hello"), "22:13:20 UTC  OFFICE  hello");
        assert_eq!(chat_name("", "OFFICE"), "OFFICE");
        assert_eq!(chat_name("ben@spader.us", "OFFICE"), "ben@spader.us");
    }

    #[tokio::test]
    async fn side_stream_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        let header = SideHeader { id: 3, offset: 100, a: 0, b: 0 };
        write_side_header(&mut a, FILE_MAGIC, &header).await.unwrap();
        write_side_chunk(&mut a, b"abcdef").await.unwrap();
        write_side_chunk(&mut a, &[]).await.unwrap();
        let (magic, got) = read_side_header::<_, SideHeader>(&mut b).await.unwrap();
        assert_eq!(&magic, FILE_MAGIC);
        assert_eq!(got.offset, 100);
        assert_eq!(read_side_chunk(&mut b).await.unwrap().unwrap(), b"abcdef");
        assert!(read_side_chunk(&mut b).await.unwrap().is_none());
    }
}

/// Writes the magic and a postcard header that introduces a side stream.
pub async fn write_side_header<W, T>(w: &mut W, magic: &[u8; 8], header: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = postcard::to_stdvec(header)?;
    if body.len() > 4096 {
        bail!("side stream header too large");
    }
    w.write_all(magic).await?;
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    Ok(())
}

/// Reads a side-stream magic and header.
pub async fn read_side_header<R, T>(r: &mut R) -> Result<([u8; 8], T)>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).await?;
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb).await?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len > 4096 {
        bail!("side stream header too large");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok((magic, postcard::from_bytes(&body)?))
}

/// Writes one chunk. An empty chunk ends the stream's payload.
pub async fn write_side_chunk<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    if data.len() > SIDE_CHUNK {
        bail!("side stream chunk too large");
    }
    w.write_all(&(data.len() as u32).to_be_bytes()).await?;
    w.write_all(data).await?;
    Ok(())
}

/// Reads one chunk. `None` is the end marker.
pub async fn read_side_chunk<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb).await?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len == 0 {
        return Ok(None);
    }
    if len > SIDE_CHUNK {
        bail!("side stream chunk too large");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}
