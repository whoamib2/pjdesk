//! H.264 encoding with Cisco's OpenH264, tuned for screen content and low
//! latency (no B-frames, no lookahead: every input frame comes out at once).

use std::time::Instant;

use anyhow::Result;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType, VuiConfig,
};
use openh264::{OpenH264API, Timestamp};

use crate::color::I420Buffer;

pub struct VideoEncoder {
    encoder: Encoder,
    frames: u64,
    started: Instant,
}

impl VideoEncoder {
    pub fn new(bitrate_kbps: u32, fps: u32) -> Result<Self> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate_kbps.max(100).saturating_mul(1000)))
            .max_frame_rate(FrameRate::from_hz(fps.clamp(1, 120) as f32))
            // OpenH264 can only hold the bitrate if it may drop frames; the
            // capture loop re-sends the latest image after a dropped frame.
            .skip_frames(true)
            // Medium spends a little more CPU for visibly sharper text and edges.
            .complexity(Complexity::Medium)
            // Not supported for screen content (OpenH264 would warn and disable them).
            .adaptive_quantization(false)
            .background_detection(false)
            // Matches the full-range BT.601 conversion in `color.rs`.
            .vui(VuiConfig::bt601().full_range(true));
        Ok(Self {
            encoder: Encoder::with_api_config(OpenH264API::from_source(), config)?,
            frames: 0,
            started: Instant::now(),
        })
    }

    /// Makes the next frame an IDR (decoder recovery point).
    pub fn request_keyframe(&mut self) {
        // The very first frame is an IDR anyway, and the encoder is only
        // initialized once it has seen a frame.
        if self.frames > 0 {
            self.encoder.force_intra_frame();
        }
    }

    /// Encodes one picture. Returns the Annex-B bitstream and whether it is a
    /// keyframe, or `None` if the encoder dropped the frame.
    pub fn encode(&mut self, picture: &I420Buffer) -> Result<Option<(Vec<u8>, bool)>> {
        // Real timestamps let the rate controller account for idle periods.
        let timestamp = Timestamp::from_millis(self.started.elapsed().as_millis() as u64);
        let bitstream = self.encoder.encode_at(picture, timestamp)?;
        let keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        let data = bitstream.to_vec();
        self.frames += 1;
        Ok((!data.is_empty()).then_some((data, keyframe)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::bgra_to_i420;
    use crate::viewer::decoder::VideoDecoder;

    #[test]
    fn encode_decode_roundtrip() {
        let (w, h) = (320usize, 240usize);
        let mut encoder = VideoEncoder::new(2000, 30).unwrap();
        let mut decoder = VideoDecoder::new().unwrap();
        let mut picture = I420Buffer::default();
        let mut rgb = Vec::new();
        for frame in 0..10u8 {
            // Left half red, right half blue, with a moving gray bar.
            let mut bgra = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    let px = if (x / 8) as u8 == frame {
                        [128, 128, 128, 255]
                    } else if x < w / 2 {
                        [0, 0, 255, 255]
                    } else {
                        [255, 0, 0, 255]
                    };
                    bgra[i..i + 4].copy_from_slice(&px);
                }
            }
            bgra_to_i420(&bgra, w, h, w * 4, 1, &mut picture);
            let (data, keyframe) = encoder.encode(&picture).unwrap().expect("frame");
            assert_eq!(keyframe, frame == 0, "only the first frame should be a keyframe");
            let size = decoder.decode(&data, &mut rgb).unwrap().expect("picture");
            assert_eq!(size, (w, h));
        }
        // Check colors far from the moving bar.
        let left = rgb[100 * w + 100];
        let right = rgb[100 * w + 260];
        assert!((left >> 16) > 200 && (left & 255) < 50, "left {left:06x}");
        assert!((right >> 16) < 50 && (right & 255) > 200, "right {right:06x}");
    }
}
