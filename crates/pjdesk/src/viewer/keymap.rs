//! Physical keys → PC/AT "set 1" scan codes.
//!
//! Sending scan codes (not characters) makes the host behave as if a
//! keyboard were plugged into it: shortcuts, games and the host's own
//! keyboard layout all work. Extended keys carry 0xE0 in the high byte.

use winit::keyboard::KeyCode;

pub fn scancode(key: KeyCode) -> Option<u16> {
    use KeyCode::*;
    Some(match key {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        Digit6 => 0x07,
        Digit7 => 0x08,
        Digit8 => 0x09,
        Digit9 => 0x0A,
        Digit0 => 0x0B,
        Minus => 0x0C,
        Equal => 0x0D,
        Backspace => 0x0E,
        Tab => 0x0F,
        KeyQ => 0x10,
        KeyW => 0x11,
        KeyE => 0x12,
        KeyR => 0x13,
        KeyT => 0x14,
        KeyY => 0x15,
        KeyU => 0x16,
        KeyI => 0x17,
        KeyO => 0x18,
        KeyP => 0x19,
        BracketLeft => 0x1A,
        BracketRight => 0x1B,
        Enter => 0x1C,
        ControlLeft => 0x1D,
        KeyA => 0x1E,
        KeyS => 0x1F,
        KeyD => 0x20,
        KeyF => 0x21,
        KeyG => 0x22,
        KeyH => 0x23,
        KeyJ => 0x24,
        KeyK => 0x25,
        KeyL => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backquote => 0x29,
        ShiftLeft => 0x2A,
        Backslash => 0x2B,
        KeyZ => 0x2C,
        KeyX => 0x2D,
        KeyC => 0x2E,
        KeyV => 0x2F,
        KeyB => 0x30,
        KeyN => 0x31,
        KeyM => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        ShiftRight => 0x36,
        NumpadMultiply => 0x37,
        AltLeft => 0x38,
        Space => 0x39,
        CapsLock => 0x3A,
        F1 => 0x3B,
        F2 => 0x3C,
        F3 => 0x3D,
        F4 => 0x3E,
        F5 => 0x3F,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        NumLock => 0x45,
        ScrollLock => 0x46,
        Numpad7 => 0x47,
        Numpad8 => 0x48,
        Numpad9 => 0x49,
        NumpadSubtract => 0x4A,
        Numpad4 => 0x4B,
        Numpad5 => 0x4C,
        Numpad6 => 0x4D,
        NumpadAdd => 0x4E,
        Numpad1 => 0x4F,
        Numpad2 => 0x50,
        Numpad3 => 0x51,
        Numpad0 => 0x52,
        NumpadDecimal => 0x53,
        IntlBackslash => 0x56,
        F11 => 0x57,
        F12 => 0x58,
        F13 => 0x64,
        F14 => 0x65,
        F15 => 0x66,
        F16 => 0x67,
        F17 => 0x68,
        F18 => 0x69,
        F19 => 0x6A,
        F20 => 0x6B,
        F21 => 0x6C,
        F22 => 0x6D,
        F23 => 0x6E,
        KanaMode => 0x70,
        IntlRo => 0x73,
        F24 => 0x76,
        Convert => 0x79,
        NonConvert => 0x7B,
        IntlYen => 0x7D,
        // Extended (0xE0-prefixed) keys.
        MediaTrackPrevious => 0xE010,
        MediaTrackNext => 0xE019,
        NumpadEnter => 0xE01C,
        ControlRight => 0xE01D,
        AudioVolumeMute => 0xE020,
        MediaPlayPause => 0xE022,
        MediaStop => 0xE024,
        AudioVolumeDown => 0xE02E,
        AudioVolumeUp => 0xE030,
        NumpadDivide => 0xE035,
        PrintScreen => 0xE037,
        AltRight => 0xE038,
        Home => 0xE047,
        ArrowUp => 0xE048,
        PageUp => 0xE049,
        ArrowLeft => 0xE04B,
        ArrowRight => 0xE04D,
        End => 0xE04F,
        ArrowDown => 0xE050,
        PageDown => 0xE051,
        Insert => 0xE052,
        Delete => 0xE053,
        SuperLeft => 0xE05B,
        SuperRight => 0xE05C,
        ContextMenu => 0xE05D,
        // Pause is the odd one out (E1 1D 45); the host sends it as a virtual key.
        Pause => 0xE11D,
        _ => return None,
    })
}

/// Numpad keys that type a digit (or the decimal separator) with NumLock on.
pub fn is_numpad_digit_key(key: KeyCode) -> bool {
    use KeyCode::*;
    matches!(
        key,
        Numpad0
            | Numpad1
            | Numpad2
            | Numpad3
            | Numpad4
            | Numpad5
            | Numpad6
            | Numpad7
            | Numpad8
            | Numpad9
            | NumpadDecimal
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn common_keys_are_mapped_uniquely() {
        let keys = [
            KeyCode::KeyA,
            KeyCode::KeyZ,
            KeyCode::Enter,
            KeyCode::NumpadEnter,
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::ArrowLeft,
            KeyCode::Numpad4,
            KeyCode::Delete,
            KeyCode::NumpadDecimal,
            KeyCode::SuperLeft,
            KeyCode::F12,
        ];
        let codes: HashSet<u16> = keys.iter().map(|k| scancode(*k).unwrap()).collect();
        assert_eq!(codes.len(), keys.len());
        assert_eq!(scancode(KeyCode::KeyA), Some(0x1E));
        assert_eq!(scancode(KeyCode::ArrowLeft), Some(0xE04B));
    }
}
