//! Clipboard text and PNG images. View-only sessions never call this.
//! File lists (CF_HDROP) are not synced; files go through file transfer.

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

/// Windows' clipboard change counter; `None` where there is none.
pub fn sequence() -> Option<u32> {
    #[cfg(windows)]
    {
        let n = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        (n != 0).then_some(n)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// A short fingerprint of image pixels, used to avoid sending an image back.
pub fn image_hash(width: usize, height: usize, rgba: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (width, height).hash(&mut h);
    rgba.hash(&mut h);
    h.finish()
}

/// Decides when clipboard text on one side should go to the other side.
///
/// The first read sets a baseline, so text that was already on the clipboard
/// when the session opened is not sent. Before this type, a failed or empty
/// first read left no baseline, and the next real copy became the baseline
/// and was silently dropped.
#[derive(Debug, Default, Clone)]
pub struct TextWatch {
    primed: bool,
    last: Option<String>,
}

impl TextWatch {
    /// Feeds one clipboard read (`None` when there was no text or it could
    /// not be read). Returns the text to send, if any.
    pub fn observe(&mut self, read: Option<String>) -> Option<String> {
        if !self.primed {
            self.primed = true;
            self.last = read;
            return None;
        }
        let text = read?;
        if text.is_empty() || self.last.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last = Some(text.clone());
        Some(text)
    }

    /// Records text that the other side put here, so it is not sent back.
    pub fn remember(&mut self, text: &str) {
        self.primed = true;
        self.last = Some(text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::TextWatch;

    #[test]
    fn a_copy_after_an_unreadable_first_read_is_sent() {
        let mut w = TextWatch::default();
        assert_eq!(w.observe(None), None);
        assert_eq!(w.observe(Some("copied on the host".into())).as_deref(), Some("copied on the host"));
    }

    #[test]
    fn text_present_at_the_start_is_not_sent_but_the_next_copy_is() {
        let mut w = TextWatch::default();
        assert_eq!(w.observe(Some("old".into())), None);
        assert_eq!(w.observe(Some("old".into())), None);
        assert_eq!(w.observe(Some("new".into())).as_deref(), Some("new"));
        assert_eq!(w.observe(Some("new".into())), None);
    }

    #[test]
    fn text_from_the_other_side_is_not_echoed() {
        let mut w = TextWatch::default();
        w.observe(Some("a".into()));
        w.remember("from the controller");
        assert_eq!(w.observe(Some("from the controller".into())), None);
        assert_eq!(w.observe(None), None);
        assert_eq!(w.observe(Some("b".into())).as_deref(), Some("b"));
    }

    #[test]
    fn remembering_before_the_first_read_primes_the_watch() {
        let mut w = TextWatch::default();
        w.remember("x");
        assert_eq!(w.observe(Some("y".into())).as_deref(), Some("y"));
    }
}
