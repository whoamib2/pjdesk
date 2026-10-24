//! Checks the generated PJDesk icon. The artwork is produced by tools/make_icon.py.

#[cfg(test)]
mod tests {
    fn corners_clear(bytes: &[u8], side: usize) {
        assert_eq!(bytes.len(), side * side * 4);
        for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
            let i = (y * side + x) * 4;
            assert_eq!(bytes[i + 3], 0, "corner {x},{y} should be transparent");
        }
    }

    #[test]
    fn window_icon_is_the_transparent_artwork() {
        let bytes = include_bytes!("../assets/icon_256.rgba");
        corners_clear(bytes, 256);
        let opaque = bytes.chunks(4).filter(|p| p[3] > 200).count();
        assert!(opaque > 8_000, "the artwork should fill the square");
        let blue = bytes
            .chunks(4)
            .filter(|p| p[3] > 200 && p[2] > 160 && p[2] > p[0].saturating_add(40))
            .count();
        assert!(blue > 200, "the cable should still be in the icon");
        let dark = bytes
            .chunks(4)
            .filter(|p| p[3] > 200 && p[0] < 30 && p[1] < 30 && p[2] < 40)
            .count();
        assert!(dark > 2_000, "the monitor glass should still be in the icon");
    }

    #[test]
    fn tray_mark_keeps_a_cursor_on_the_glass() {
        let bytes = include_bytes!("../assets/icon_32.rgba");
        corners_clear(bytes, 32);
        let dark = bytes
            .chunks(4)
            .filter(|p| p[3] > 180 && p[0] < 40 && p[1] < 40 && p[2] < 50)
            .count();
        let light = bytes
            .chunks(4)
            .filter(|p| p[3] > 180 && p[0] > 210 && p[1] > 210 && p[2] > 210)
            .count();
        assert!(dark > 80, "the glass has to survive at tray size");
        assert!(light > 30, "the cursor or the frame has to survive at tray size");
    }

    #[test]
    fn windows_icon_carries_16_through_256() {
        let ico = include_bytes!("../assets/pjdesk.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        assert_eq!(count, 7);
        let mut sizes = Vec::new();
        for i in 0..count {
            let w = ico[6 + i * 16];
            sizes.push(if w == 0 { 256 } else { u16::from(w) });
            let size = u32::from_le_bytes(ico[6 + i * 16 + 8..6 + i * 16 + 12].try_into().unwrap());
            let offset = u32::from_le_bytes(ico[6 + i * 16 + 12..6 + i * 16 + 16].try_into().unwrap()) as usize;
            assert!(size > 32, "image {i} is empty");
            assert_eq!(&ico[offset..offset + 4], b"\x89PNG", "image {i} should be a PNG");
        }
        assert_eq!(sizes, vec![16, 24, 32, 48, 64, 128, 256]);
    }

    #[test]
    fn server_uses_the_same_icon() {
        assert_eq!(
            include_bytes!("../assets/pjdesk.ico") as &[u8],
            include_bytes!("../../pjdesk-server/assets/pjdesk-server.ico")
        );
        assert_eq!(
            include_bytes!("../assets/icon_256.rgba") as &[u8],
            include_bytes!("../../pjdesk-server/assets/icon_server_256.rgba")
        );
        assert_eq!(
            include_bytes!("../assets/icon_32.rgba") as &[u8],
            include_bytes!("../../pjdesk-server/assets/icon_server_32.rgba")
        );
    }
}
