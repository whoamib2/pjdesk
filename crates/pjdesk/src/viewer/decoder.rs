//! H.264 decoding (OpenH264) straight into 0RGB pixels for the window.

use anyhow::Result;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

use crate::color::i420_to_rgb32;

pub struct VideoDecoder {
    decoder: Decoder,
}

impl VideoDecoder {
    pub fn new() -> Result<Self> {
        Ok(Self { decoder: Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())? })
    }

    /// Decodes one access unit into `out` (resized as needed). Returns the
    /// picture size, or `None` if the data produced no picture.
    pub fn decode(&mut self, data: &[u8], out: &mut Vec<u32>) -> Result<Option<(usize, usize)>> {
        let Some(picture) = self.decoder.decode(data)? else { return Ok(None) };
        let (width, height) = picture.dimensions();
        let (y_stride, u_stride, _) = picture.strides();
        out.resize(width * height, 0);
        i420_to_rgb32(picture.y(), picture.u(), picture.v(), y_stride, u_stride, width, height, out);
        Ok(Some((width, height)))
    }
}
