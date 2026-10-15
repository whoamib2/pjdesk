//! A moving test pattern, for testing streaming without a real desktop.
//!
//! Layout (left to right): red, green and blue bars, then an animated
//! gradient with a white box sweeping across. The fixed bars let automated
//! tests check colors after the full encode → network → decode trip.

use std::time::Duration;

use anyhow::Result;

use super::{Capturer, Frame, Rect};

pub struct Synthetic {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    tick: u32,
}

impl Synthetic {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![0; width * height * 4], tick: 0 }
    }

    fn render(&mut self) {
        let (w, h, t) = (self.width, self.height, self.tick as usize);
        let quarter = w / 4;
        let box_size = h / 4;
        let box_x = quarter * 3 + (t * 8) % (w - quarter * 3 - box_size).max(1);
        let box_y = h / 2 - box_size / 2;
        for y in 0..h {
            let row = &mut self.pixels[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let (r, g, b) = if x < quarter {
                    (230, 20, 20)
                } else if x < quarter * 2 {
                    (20, 230, 20)
                } else if x < quarter * 3 {
                    (20, 20, 230)
                } else if (box_x..box_x + box_size).contains(&x) && (box_y..box_y + box_size).contains(&y) {
                    (255, 255, 255)
                } else {
                    (((x + t * 4) % 256) as u8, ((y + t * 2) % 256) as u8, 96)
                };
                row[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        self.tick = self.tick.wrapping_add(1);
    }

    fn frame(&self) -> Frame<'_> {
        Frame { data: &self.pixels, width: self.width, height: self.height, stride: self.width * 4 }
    }
}

impl Capturer for Synthetic {
    fn name(&self) -> &'static str {
        "the synthetic test pattern"
    }

    fn display_rect(&self) -> Rect {
        Rect { left: 0, top: 0, width: self.width as i32, height: self.height as i32 }
    }

    fn capture(&mut self, _timeout: Duration) -> Result<Option<Frame<'_>>> {
        self.render();
        Ok(Some(self.frame()))
    }
}
