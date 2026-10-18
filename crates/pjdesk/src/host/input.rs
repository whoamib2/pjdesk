//! Replays the controller's input on the host with `SendInput`.
//!
//! Mouse positions arrive normalized to the shared display and are mapped to
//! that monitor's rectangle in the virtual desktop. Keys arrive as hardware
//! scan codes and are injected as scan codes, so Windows applies the keyboard
//! layout of the focused app, as if a keyboard were plugged into the host.
//!
//! Limits of `SendInput`: it can't reach elevated windows unless PJDesk runs
//! as Administrator, can't type into the secure desktop (UAC, lock screen), and
//! can't send Ctrl+Alt+Del.

use std::collections::HashSet;

use pjdesk_proto::peer::{InputEvent, MouseButton};

use super::capture::Rect;

pub struct InputInjector {
    allow_mouse: bool,
    allow_keyboard: bool,
    keys_down: HashSet<u16>,
    buttons_down: HashSet<MouseButton>,
}

impl InputInjector {
    pub fn new(view_only: bool) -> Self {
        Self {
            allow_mouse: !view_only,
            allow_keyboard: !view_only,
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
        }
    }

    pub fn set_allowed(&mut self, mouse: bool, keyboard: bool) {
        self.allow_mouse = mouse;
        self.allow_keyboard = keyboard;
        if !mouse || !keyboard {
            self.release_all();
        }
    }

    /// Unicode entry for Translate keyboard mode. Ignored without keyboard permission.
    pub fn text(&mut self, chars: &str) {
        if !self.allow_keyboard {
            return;
        }
        for ch in chars.chars().take(64) {
            platform::unicode(ch);
        }
    }

    pub fn handle(&mut self, event: InputEvent, display: Rect) {
        match event {
            InputEvent::MouseMove { x, y } => {
                if self.allow_mouse {
                    platform::mouse_move(display, x, y);
                }
            }
            InputEvent::MouseButton { button, down } => {
                if !self.allow_mouse {
                    return;
                }
                // Track the state Windows actually has: input is rejected
                // while the secure desktop (UAC, lock screen) is showing, so a
                // release may need to be retried later by `release_all`.
                if platform::mouse_button(button, down) {
                    self.buttons_down.insert(button);
                } else {
                    self.buttons_down.remove(&button);
                }
            }
            InputEvent::Wheel { dx, dy } => {
                if self.allow_mouse {
                    platform::wheel(dx, dy);
                }
            }
            InputEvent::Key { scan, down } => {
                if !self.allow_keyboard {
                    return;
                }
                let sent = platform::key(scan, down);
                if down && sent {
                    self.keys_down.insert(scan);
                } else if !down && sent {
                    self.keys_down.remove(&scan);
                }
            }
            InputEvent::ReleaseAll => self.release_all(),
        }
    }

    /// Releases everything the controller left pressed (no stuck Ctrl keys).
    pub fn release_all(&mut self) {
        self.keys_down.retain(|&scan| !platform::key(scan, false));
        self.buttons_down.retain(|&button| platform::mouse_button(button, false));
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::OnceLock;

    use pjdesk_proto::peer::{MouseButton, KEY_NUMPAD_DIGIT};
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    use super::Rect;

    /// Sends the inputs; true if Windows accepted all of them.
    fn send(inputs: &[INPUT]) -> bool {
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
    }

    fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    /// Normalized coordinate (0..=65535) that Windows maps back to pixel
    /// `offset` of a `size`-pixel axis (pixel = n * size / 65536).
    fn normalize(offset: i64, size: i64) -> i32 {
        (((2 * offset + 1) * 65536) / (2 * size)).clamp(0, 65535) as i32
    }

    pub fn mouse_move(display: Rect, x: u16, y: u16) -> bool {
        if display.width <= 0 || display.height <= 0 {
            return false;
        }
        let px = display.left as i64 + (x as i64 * (display.width as i64 - 1) + 32767) / 65535;
        let py = display.top as i64 + (y as i64 * (display.height as i64 - 1) + 32767) / 65535;
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN) as i64,
                GetSystemMetrics(SM_YVIRTUALSCREEN) as i64,
                GetSystemMetrics(SM_CXVIRTUALSCREEN) as i64,
                GetSystemMetrics(SM_CYVIRTUALSCREEN) as i64,
            )
        };
        if vw <= 0 || vh <= 0 {
            return false;
        }
        send(&[mouse(
            normalize(px - vx, vw),
            normalize(py - vy, vh),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )])
    }

    /// Returns the button's resulting state as Windows saw it (`down` if
    /// accepted, the opposite if the input was rejected).
    pub fn mouse_button(button: MouseButton, down: bool) -> bool {
        const XBUTTON1: i32 = 1;
        const XBUTTON2: i32 = 2;
        let (flags, data) = match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        if send(&[mouse(0, 0, data, flags)]) {
            down
        } else {
            !down
        }
    }

    pub fn wheel(dx: i16, dy: i16) {
        if dy != 0 {
            send(&[mouse(0, 0, dy as i32, MOUSEEVENTF_WHEEL)]);
        }
        if dx != 0 {
            send(&[mouse(0, 0, dx as i32, MOUSEEVENTF_HWHEEL)]);
        }
    }

    fn keyboard(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    /// Wine ignores scan-code-only keyboard input, so there we send virtual keys.
    fn under_wine() -> bool {
        static WINE: OnceLock<bool> = OnceLock::new();
        *WINE.get_or_init(|| unsafe {
            GetModuleHandleW(w!("ntdll.dll"))
                .map(|ntdll| GetProcAddress(ntdll, s!("wine_get_version")).is_some())
                .unwrap_or(false)
        })
    }

    /// Virtual key for a numpad scan code typed with NumLock on.
    fn numpad_digit_vk(scan: u8) -> Option<VIRTUAL_KEY> {
        Some(match scan {
            0x52 => VK_NUMPAD0,
            0x4F => VK_NUMPAD1,
            0x50 => VK_NUMPAD2,
            0x51 => VK_NUMPAD3,
            0x4B => VK_NUMPAD4,
            0x4C => VK_NUMPAD5,
            0x4D => VK_NUMPAD6,
            0x47 => VK_NUMPAD7,
            0x48 => VK_NUMPAD8,
            0x49 => VK_NUMPAD9,
            0x53 => VK_DECIMAL,
            _ => return None,
        })
    }

    /// Injects one key event; true if Windows accepted it.
    pub fn key(code: u16, down: bool) -> bool {
        let up = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        let input = if code & KEY_NUMPAD_DIGIT != 0 {
            // The controller typed a digit on its numpad (NumLock on there).
            // Send the digit itself so it doesn't depend on the host's NumLock.
            let scan = (code & 0xFF) as u8;
            match numpad_digit_vk(scan) {
                Some(vk) => keyboard(vk, scan as u16, up),
                None => return false,
            }
        } else {
            match code {
                // Keys whose scan codes don't round-trip through SendInput: virtual keys.
                0xE11D => keyboard(VK_PAUSE, 0x45, up),
                0x0045 => keyboard(VK_NUMLOCK, 0x45, up | KEYEVENTF_EXTENDEDKEY),
                0xE037 => keyboard(VK_SNAPSHOT, 0x37, up | KEYEVENTF_EXTENDEDKEY),
                _ => {
                    let mut flags = up;
                    if code & 0xFF00 == 0xE000 {
                        flags |= KEYEVENTF_EXTENDEDKEY;
                    }
                    if under_wine() {
                        let vk = unsafe { MapVirtualKeyW(code as u32, MAPVK_VSC_TO_VK_EX) } as u16;
                        keyboard(VIRTUAL_KEY(vk), code & 0xFF, flags)
                    } else {
                        // Windows translates the scan code with the focused app's layout.
                        keyboard(VIRTUAL_KEY(0), code & 0xFF, flags | KEYEVENTF_SCANCODE)
                    }
                }
            }
        };
        send(&[input])
    }

    /// Unicode entry for Translate mode. Windows applies the character itself,
    /// so the host keyboard layout does not matter.
    pub fn unicode(ch: char) -> bool {
        if ch.is_control() {
            return false;
        }
        let mut units = [0u16; 2];
        let encoded = ch.encode_utf16(&mut units);
        encoded.iter().all(|&unit| {
            let down = keyboard(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE);
            let up = keyboard(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
            send(&[down, up])
        })
    }
}

#[cfg(not(windows))]
mod platform {
    //! Input injection is only implemented for Windows hosts; elsewhere input
    //! is logged and ignored (the test pattern host uses this).
    use pjdesk_proto::peer::MouseButton;

    use super::Rect;

    pub fn mouse_move(_display: Rect, _x: u16, _y: u16) -> bool {
        true
    }

    pub fn mouse_button(button: MouseButton, down: bool) -> bool {
        tracing::debug!("mouse {button:?} {}", if down { "down" } else { "up" });
        down
    }

    pub fn wheel(dx: i16, dy: i16) {
        tracing::debug!("wheel {dx} {dy}");
    }

    pub fn key(scan: u16, down: bool) -> bool {
        tracing::debug!("key {scan:#06x} {}", if down { "down" } else { "up" });
        true
    }

    pub fn unicode(ch: char) -> bool {
        tracing::debug!("unicode {ch}");
        true
    }
}
