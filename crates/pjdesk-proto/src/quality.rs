//! Stream quality presets and a small adaptive-bitrate controller.
//!
//! The host starts at the user's target. If frame acknowledgements come back
//! late it steps the bitrate down; if they stay quick it steps back toward
//! the target. The codec stays H.264 — that is the encoder both sides have.

/// One quality choice the controller can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamQuality {
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub max_width: u32,
}

impl StreamQuality {
    pub fn clamp(self) -> Self {
        Self {
            fps: self.fps.clamp(1, 60),
            bitrate_kbps: self.bitrate_kbps.clamp(200, 50_000),
            max_width: self.max_width.clamp(640, 3840),
        }
    }
}

impl Default for StreamQuality {
    fn default() -> Self {
        Self { fps: 30, bitrate_kbps: 6_000, max_width: 1920 }
    }
}

/// Named presets shown in the session toolbar.
pub fn preset(name: &str) -> Option<StreamQuality> {
    Some(match name {
        "smooth" => StreamQuality { fps: 24, bitrate_kbps: 2_000, max_width: 1280 },
        "balanced" => StreamQuality { fps: 30, bitrate_kbps: 6_000, max_width: 1920 },
        "sharp" => StreamQuality { fps: 30, bitrate_kbps: 12_000, max_width: 2560 },
        "high" => StreamQuality { fps: 60, bitrate_kbps: 20_000, max_width: 2560 },
        _ => return None,
    })
}

/// Tracks the bitrate actually used versus the user's target.
#[derive(Debug, Clone)]
pub struct BitrateControl {
    pub target: u32,
    pub current: u32,
    min: u32,
    good: u8,
    bad: u8,
}

impl BitrateControl {
    pub fn new(target_kbps: u32) -> Self {
        let target = target_kbps.clamp(200, 50_000);
        Self { target, current: target, min: 200, good: 0, bad: 0 }
    }

    pub fn set_target(&mut self, target_kbps: u32) {
        self.target = target_kbps.clamp(200, 50_000);
        if self.current > self.target {
            self.current = self.target;
        }
        self.good = 0;
        self.bad = 0;
    }

    /// `delay_ms` is how long a frame waited to be acknowledged.
    /// Returns true when `current` changed.
    pub fn observe(&mut self, delay_ms: u32) -> bool {
        let before = self.current;
        if delay_ms >= 180 {
            self.bad = self.bad.saturating_add(1);
            self.good = 0;
            if self.bad >= 2 {
                self.current = ((self.current as u64) * 4 / 5).max(self.min as u64) as u32;
                self.bad = 0;
            }
        } else if delay_ms <= 45 {
            self.good = self.good.saturating_add(1);
            self.bad = 0;
            if self.good >= 8 && self.current < self.target {
                let step = (self.current / 10).max(100);
                self.current = self.current.saturating_add(step).min(self.target);
                self.good = 0;
            }
        } else {
            self.good = 0;
            self.bad = 0;
        }
        self.current != before
    }
}

/// Linear resample of interleaved stereo s16 samples. Used so audio always
/// crosses the wire at 48 kHz even when the host device does not.
pub fn resample_s16_interleaved(input: &[i16], channels: usize, in_rate: u32, out_rate: u32) -> Vec<i16> {
    if channels == 0 || in_rate == 0 || out_rate == 0 || input.len() < channels {
        return Vec::new();
    }
    let frames = input.len() / channels;
    if in_rate == out_rate {
        return input[..frames * channels].to_vec();
    }
    let out_frames = ((frames as u64) * out_rate as u64 / in_rate as u64).max(1) as usize;
    let mut out = vec![0i16; out_frames * channels];
    for i in 0..out_frames {
        let src = (i as u64 * in_rate as u64) as f64 / out_rate as f64;
        let i0 = (src.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let frac = (src - i0 as f64) as f32;
        for c in 0..channels {
            let a = input[i0 * channels + c] as f32;
            let b = input[i1 * channels + c] as f32;
            out[i * channels + c] = (a + (b - a) * frac) as i16;
        }
    }
    out
}

pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u8 = 2;
/// 20 ms at 48 kHz, stereo s16.
pub const AUDIO_FRAME_SAMPLES: usize = 960 * 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_within_bounds() {
        for name in ["smooth", "balanced", "sharp", "high"] {
            let q = preset(name).unwrap().clamp();
            assert!(q.fps >= 1 && q.bitrate_kbps >= 200);
        }
        assert!(preset("av1").is_none());
    }

    #[test]
    fn slow_acks_lower_bitrate_and_fast_acks_recover() {
        let mut c = BitrateControl::new(6_000);
        assert!(!c.observe(20));
        // One late frame is not enough; two in a row step the bitrate down.
        assert!(!c.observe(250));
        assert!(c.observe(250));
        assert!(c.current < 6_000);
        let dipped = c.current;
        for _ in 0..40 {
            c.observe(20);
        }
        assert!(c.current > dipped);
        assert!(c.current <= c.target);
    }

    #[test]
    fn resample_identity_and_upsample() {
        let src: Vec<i16> = (0..8).map(|i| i * 1000).collect();
        assert_eq!(resample_s16_interleaved(&src, 2, 48_000, 48_000), src);
        let up = resample_s16_interleaved(&src, 2, 24_000, 48_000);
        assert!(up.len() >= src.len());
        assert_eq!(up[0], src[0]);
    }
}
