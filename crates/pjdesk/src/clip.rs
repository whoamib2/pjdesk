//! Clipboard text and PNG images. View-only sessions never call this.
//! File lists (CF_HDROP) are not synced.

use std::io::Cursor;

/// Puts text on the local clipboard. Failures are ignored: a missing
/// clipboard must not drop the remote session.
pub fn set_text(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

pub fn get_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

pub fn set_image(width: usize, height: usize, rgba: &[u8]) {
    if rgba.len() < width.saturating_mul(height).saturating_mul(4) || width == 0 || height == 0 {
        return;
    }
    let image = arboard::ImageData {
        width,
        height,
        bytes: std::borrow::Cow::Borrowed(rgba),
    };
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_image(image);
    }
}

pub fn get_image() -> Option<(usize, usize, Vec<u8>)> {
    let image = arboard::Clipboard::new().ok()?.get_image().ok()?;
    Some((image.width, image.height, image.bytes.into_owned()))
}

pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

pub fn decode_png(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.chunks(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            rgba
        }
        _ => return None,
    };
    Some((info.width as usize, info.height as usize, rgba))
}
