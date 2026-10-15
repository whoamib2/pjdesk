//! Replays the controller's input on the host with `SendInput`.
//!
//! Mouse positions arrive normalized to the shared display and are mapped to
//! that monitor's rectangle in the virtual desktop. Keys arrive as hardware
//! scan codes, so the host's own keyboard layout applies (like a USB keyboard
//! plugged into the host).
//!
//! Limits of `SendInput`: it can't reach elevated windows unless DeskLink runs
//! as Administrator, can't type into the secure desktop (UAC, lock screen), and
//! can't send Ctrl+Alt+Del.

use std::collections::HashSet;

use desklink_proto::peer::{InputEvent, MouseButton};

use super::capture::Rect;

pub struct InputInjector {
    view_only: bool,
    keys_down: HashSet<u16>,
    buttons_down: HashSet<MouseButton>,
}

impl InputInjector {
    pub fn new(view_only: bool) -> Self {
        Self { view_only, keys_down: HashSet::new(), buttons_down: HashSet::new() }
    }

    pub fn handle(&mut self, event: InputEvent, display: Rect) {
        if self.view_only {
            return;
        }
        match event {
            InputEvent::MouseMove { x, y } => platform::mouse_move(display, x, y),
            InputEvent::MouseButton { button, down } => {
                if down {
                    self.buttons_down.insert(button);
                } else {
                    self.buttons_down.remove(&button);
                }
                platform::mouse_button(button, down);
            }
            InputEvent::Wheel { dx, dy } => platform::wheel(dx, dy),
            InputEvent::Key { scan, down } => {
                if down {
                    self.keys_down.insert(scan);
                } else {
                    self.keys_down.remove(&scan);
                }
                platform::key(scan, down);
            }
            InputEvent::ReleaseAll => self.release_all(),
        }
    }

    /// Releases everything the controller left pressed (no stuck Ctrl keys).
    pub fn release_all(&mut self) {
        for scan in self.keys_down.drain() {
            platform::key(scan, false);
        }
        for button in self.buttons_down.drain() {
            platform::mouse_button(button, false);
        }
    }
}

#[cfg(windows)]
mod platform {
    use desklink_proto::peer::MouseButton;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    use super::Rect;

    fn send(inputs: &[INPUT]) {
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
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

    pub fn mouse_move(display: Rect, x: u16, y: u16) {
        if display.width <= 0 || display.height <= 0 {
            return;
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
            return;
        }
        send(&[mouse(
            normalize(px - vx, vw),
            normalize(py - vy, vh),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )]);
    }

    pub fn mouse_button(button: MouseButton, down: bool) {
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
        send(&[mouse(0, 0, data, flags)]);
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

    pub fn key(scan: u16, down: bool) {
        let up = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        let input = match scan {
            // Keys whose scan codes don't round-trip through SendInput: use virtual keys.
            0xE11D => keyboard(VK_PAUSE, 0x45, up),
            0x0045 => keyboard(VK_NUMLOCK, 0x45, up | KEYEVENTF_EXTENDEDKEY),
            0xE037 => keyboard(VK_SNAPSHOT, 0x37, up | KEYEVENTF_EXTENDEDKEY),
            _ => {
                let mut flags = up;
                if scan & 0xFF00 == 0xE000 {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                // Translate with the host's keyboard layout, like a physical
                // keyboard driver would, and pass the scan code along too.
                let vk = unsafe { MapVirtualKeyW(scan as u32, MAPVK_VSC_TO_VK_EX) } as u16;
                if vk == 0 {
                    flags |= KEYEVENTF_SCANCODE; // unknown to the layout: let Windows decide
                }
                keyboard(VIRTUAL_KEY(vk), scan & 0x00FF, flags)
            }
        };
        send(&[input]);
    }
}

#[cfg(not(windows))]
mod platform {
    //! Input injection is only implemented for Windows hosts; elsewhere input
    //! is logged and ignored (the test pattern host uses this).
    use desklink_proto::peer::MouseButton;

    use super::Rect;

    pub fn mouse_move(_display: Rect, _x: u16, _y: u16) {}

    pub fn mouse_button(button: MouseButton, down: bool) {
        tracing::debug!("mouse {button:?} {}", if down { "down" } else { "up" });
    }

    pub fn wheel(dx: i16, dy: i16) {
        tracing::debug!("wheel {dx} {dy}");
    }

    pub fn key(scan: u16, down: bool) {
        tracing::debug!("key {scan:#06x} {}", if down { "down" } else { "up" });
    }
}
