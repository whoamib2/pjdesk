//! Per-session permission bits.
//!
//! A session may only do what the controller asked for, what the person at the
//! computer allowed, and what the server policy still permits. View-only
//! sessions lose every bit that can change the host or read its clipboard.

/// Keyboard input, including special keys such as Ctrl+Alt+Del.
pub const KEYBOARD: u32 = 1 << 0;
/// Mouse movement, buttons and the wheel.
pub const MOUSE: u32 = 1 << 1;
/// Reading or writing the host clipboard. A view-only session never gets this.
pub const CLIPBOARD: u32 = 1 << 2;
/// Sending or receiving files. Writes stay inside the host's chosen folder.
pub const FILE: u32 = 1 << 3;
/// Host system-audio forwarded to the controller.
pub const AUDIO: u32 = 1 << 4;
/// Asking Windows to restart. The controller then tries to reconnect.
pub const RESTART: u32 = 1 << 5;
/// Blanking the host screen while the session is connected.
pub const PRIVACY: u32 = 1 << 6;
/// Stopping local keyboard and mouse on the host.
pub const BLOCK_INPUT: u32 = 1 << 7;
/// Text chat during the session.
pub const CHAT: u32 = 1 << 8;
/// The controller may record the session it is already watching.
pub const RECORD: u32 = 1 << 9;
/// TCP port forwards. Off unless the host listed the port.
pub const TUNNEL: u32 = 1 << 10;

/// Not a permission. A 0.5.2 controller sets this on Hello so the host knows
/// it can send display-layout and platform messages. It is intentionally
/// absent from [`ALL`], so [`grant`] clears it and an older host never sees it
/// as something the person allowed.
pub const LAYOUT: u32 = 1 << 11;

/// Not a permission. A 0.6.8 controller sets this on Hello so the host knows
/// it can ask who is connecting. Absent from [`ALL`], so [`grant`] clears it
/// and an older host never treats it as something the person allowed.
pub const VISITOR: u32 = 1 << 12;

/// Not a permission. A 0.6.50 controller sets this on Hello so the host knows
/// it can send [`crate::peer::HostMsg::ChatReady`] and named chat lines.
/// Absent from [`ALL`], so [`grant`] clears it and an older host never treats
/// it as something the person allowed.
pub const CHAT_PEER: u32 = 1 << 13;

/// Every permission bit this version defines. Unknown high bits are ignored.
pub const ALL: u32 =
    KEYBOARD | MOUSE | CLIPBOARD | FILE | AUDIO | RESTART | PRIVACY | BLOCK_INPUT | CHAT | RECORD | TUNNEL;

/// What a version-2 controller is allowed: the original remote desktop.
pub const CLASSIC: u32 = KEYBOARD | MOUSE;

/// What an unattended (nobody at the screen) session gets unless configured otherwise.
pub const UNATTENDED_DEFAULT: u32 = KEYBOARD | MOUSE;

/// Removed whenever the host is in view-only mode. Clipboard is in this mask
/// on purpose: a view-only session must not read or write it.
pub const VIEW_ONLY_DENIED: u32 = KEYBOARD | MOUSE | CLIPBOARD | FILE | RESTART | PRIVACY | BLOCK_INPUT | TUNNEL;

/// `requested` ∩ `ceiling`, then view-only stripping.
pub fn grant(requested: u32, ceiling: u32, view_only: bool) -> u32 {
    let mut g = requested & ceiling & ALL;
    if view_only {
        g &= !VIEW_ONLY_DENIED;
    }
    g
}

pub fn has(perms: u32, bit: u32) -> bool {
    perms & bit == bit
}

/// A short label for a bit, for audit lines and the approval dialog.
pub fn name(bit: u32) -> &'static str {
    match bit {
        KEYBOARD => "keyboard",
        MOUSE => "mouse",
        CLIPBOARD => "clipboard",
        FILE => "file transfer",
        AUDIO => "audio",
        RESTART => "restart",
        PRIVACY => "privacy mode",
        BLOCK_INPUT => "block input",
        CHAT => "chat",
        RECORD => "recording",
        TUNNEL => "port forward",
        _ => "unknown",
    }
}

/// Human list of the bits that are on, in a stable order.
pub fn describe(perms: u32) -> String {
    let bits = [KEYBOARD, MOUSE, CLIPBOARD, FILE, AUDIO, RESTART, PRIVACY, BLOCK_INPUT, CHAT, RECORD, TUNNEL];
    let names: Vec<&str> = bits.iter().copied().filter(|b| has(perms, *b)).map(name).collect();
    if names.is_empty() {
        "view only".into()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_only_drops_clipboard_and_input() {
        let asked = ALL;
        let got = grant(asked, ALL, true);
        assert!(!has(got, CLIPBOARD));
        assert!(!has(got, KEYBOARD));
        assert!(!has(got, MOUSE));
        assert!(!has(got, FILE));
        assert!(!has(got, RESTART));
        assert!(!has(got, TUNNEL));
        assert!(has(got, CHAT));
        assert!(has(got, AUDIO));
        assert!(has(got, RECORD));
    }

    #[test]
    fn ceiling_and_request_intersect() {
        let got = grant(KEYBOARD | MOUSE | FILE, KEYBOARD | CHAT, false);
        assert_eq!(got, KEYBOARD);
    }

    #[test]
    fn unknown_bits_are_cleared() {
        assert_eq!(grant(1 << 30 | MOUSE, ALL, false), MOUSE);
    }

    #[test]
    fn layout_bit_is_not_a_permission() {
        assert_eq!(grant(ALL | LAYOUT, ALL | LAYOUT, false), ALL);
        assert!(!has(ALL, LAYOUT));
        assert_eq!(name(LAYOUT), "unknown");
    }

    #[test]
    fn chat_peer_bit_is_not_a_permission() {
        assert_eq!(grant(ALL | CHAT_PEER, ALL | CHAT_PEER, false), ALL);
        assert!(!has(ALL, CHAT_PEER));
        assert_eq!(name(CHAT_PEER), "unknown");
        assert!(has(ALL, CHAT));
    }
}
