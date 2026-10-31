//! Side panel for monitors and port forwards. Files and chat have their own windows.
//!
//! Drawn into the same framebuffer as the picture, so the viewer stays one window.

use super::controls::Live;
use super::toolbar::{draw_text, fill_rect};

const ROW: f32 = 26.0;

pub struct Panel {
    pub open: bool,
    pub tab: u8,
    pub draft: String,
    pub path: String,
    pub local_port: String,
    pub remote_host: String,
    pub remote_port: String,
    /// 0 none, 1 chat, 2 file path, 3 local port.
    pub focus: u8,
    pub confirm_restart: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            tab: 0,
            draft: String::new(),
            path: String::new(),
            local_port: "2222".into(),
            remote_host: "127.0.0.1".into(),
            remote_port: "22".into(),
            focus: 0,
            confirm_restart: false,
        }
    }
}

pub enum PanelCmd {
    SendFile(String),
    Accept(u64),
    Pull(String),
    List,
    Chat(String),
    Tunnel { local: u16, host: String, port: u16 },
    Monitor(u8),
}

impl Panel {
    pub fn rect(&self, win_w: usize, win_h: usize) -> Option<(f32, f32, f32, f32)> {
        if !self.open || win_w < 200 || win_h < 120 {
            return None;
        }
        let w = 360.0_f32.min(win_w as f32 * 0.46);
        Some((win_w as f32 - w - 10.0, 46.0, w, (win_h as f32 - 58.0).max(80.0)))
    }

    pub fn covers(&self, x: f64, y: f64, win_w: usize, win_h: usize) -> bool {
        let Some((rx, ry, rw, rh)) = self.rect(win_w, win_h) else { return false };
        x >= rx as f64 && x <= (rx + rw) as f64 && y >= ry as f64 && y <= (ry + rh) as f64
    }

    pub fn type_text(&mut self, text: &str) {
        let field = match self.focus {
            1 => &mut self.draft,
            2 => &mut self.path,
            3 => &mut self.local_port,
            _ => return,
        };
        let limit = if self.focus == 1 { pjdesk_proto::peer::MAX_CHAT_TEXT } else { 240 };
        if field.chars().count() < limit {
            field.push_str(text);
        }
    }

    pub fn backspace(&mut self) {
        let field = match self.focus {
            1 => &mut self.draft,
            2 => &mut self.path,
            3 => &mut self.local_port,
            _ => return,
        };
        field.pop();
    }

    pub fn click(&mut self, x: f64, y: f64, win_w: usize, win_h: usize, live: &Live) -> Option<PanelCmd> {
        let (rx, ry, rw, _rh) = self.rect(win_w, win_h)?;
        let lx = x as f32 - rx;
        let ly = y as f32 - ry;
        let _ = rw;
        if ly < ROW {
            self.focus = 0;
            return None;
        }
        self.click_more(lx, ly, live)
    }

    fn click_chat(&mut self, lx: f32, ly: f32, rw: f32) -> Option<PanelCmd> {
        if ly > rw {
            // The input row is near the bottom; the caller passes height via a second check.
        }
        let _ = (lx, rw);
        self.focus = 1;
        None
    }

    pub fn submit_chat(&mut self) -> Option<PanelCmd> {
        let text = self.draft.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.draft.clear();
        Some(PanelCmd::Chat(text))
    }

    fn click_more(&mut self, lx: f32, ly: f32, live: &Live) -> Option<PanelCmd> {
        let mut y = ROW + 8.0;
        for mon in &live.monitors {
            if (y..y + ROW).contains(&ly) {
                return Some(PanelCmd::Monitor(mon.index));
            }
            y += ROW;
        }
        y += ROW;
        if (y..y + ROW).contains(&ly) {
            self.focus = 3;
            return None;
        }
        y += ROW;
        if (y..y + ROW).contains(&ly) && lx < 140.0 {
            let local = self.local_port.parse().unwrap_or(0);
            let port = self.remote_port.parse().unwrap_or(0);
            if local > 0 && port > 0 {
                return Some(PanelCmd::Tunnel {
                    local,
                    host: self.remote_host.clone(),
                    port,
                });
            }
        }
        None
    }

    pub fn draw(&self, buf: &mut [u32], win_w: usize, win_h: usize, live: &Live) {
        let Some((x, y, w, h)) = self.rect(win_w, win_h) else { return };
        fill_rect(buf, win_w, win_h, x, y, w, h, 0xF2_16_1b_22);
        fill_rect(buf, win_w, win_h, x, y, 3.0, h, 0xFF_4d_9b_ff);
        draw_text(buf, win_w, win_h, x + 12.0, y + 18.0, "Session", 0xFF_ff_ff_ff);
        fill_rect(buf, win_w, win_h, x, y + ROW, w, 1.0, 0xFF_2a_30_3b);
        self.draw_more(buf, win_w, win_h, x, y, w, live);
        if let Some(note) = live.notices.last() {
            let shown: String = note.chars().take(64).collect();
            draw_text(buf, win_w, win_h, x + 12.0, y + h - 10.0, &shown, 0xFF_f2_b0_3a);
        }
    }

    fn draw_chat(&self, buf: &mut [u32], win_w: usize, win_h: usize, x: f32, y: f32, w: f32, h: f32, live: &Live) {
        let mut row = y + ROW + 8.0;
        if !live.chat_ready {
            draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, pjdesk_proto::peer::chat_off_tip(&live.peer_version), 0xFF_f2_b0_3a);
            return;
        }
        let start = live.chat.len().saturating_sub(12);
        for line in live.chat.iter().skip(start) {
            let text = pjdesk_proto::peer::format_chat_line(line.unix, &line.name, &line.text);
            draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, &clip(&text, 48), if line.mine { 0xFF_cf_e3_ff } else { 0xFF_e8_ec_f2 });
            row += ROW;
        }
        let input_y = y + h - 46.0;
        fill_rect(buf, win_w, win_h, x + 8.0, input_y, w - 16.0, 28.0, 0xFF_26_2c_37);
        let shown = if self.draft.is_empty() { "Message, then Enter" } else { self.draft.as_str() };
        draw_text(buf, win_w, win_h, x + 14.0, input_y + 18.0, &clip(shown, 42), 0xFF_e8_ec_f2);
    }

    fn draw_more(&self, buf: &mut [u32], win_w: usize, win_h: usize, x: f32, y: f32, _w: f32, live: &Live) {
        let mut row = y + ROW + 8.0;
        draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, "Monitors", 0xFF_9a_a4_b5);
        row += ROW;
        if live.monitors.is_empty() {
            draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, "Waiting for the host…", 0xFF_9a_a4_b5);
            row += ROW;
        }
        for mon in &live.monitors {
            let label = format!("{}  {}x{} {}", mon.index + 1, mon.width, mon.height, if mon.primary { "primary" } else { "" });
            draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, &clip(&label, 40), 0xFF_e8_ec_f2);
            row += ROW;
        }
        row += 8.0;
        draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, "Port forward to the host", 0xFF_9a_a4_b5);
        row += ROW;
        let line = format!("localhost:{}  →  {}:{}", self.local_port, self.remote_host, self.remote_port);
        draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, &clip(&line, 42), 0xFF_e8_ec_f2);
        row += ROW;
        draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, "Start forward", 0xFF_4d_9b_ff);
        row += ROW + 8.0;
        let bits = pjdesk_proto::perms::describe(live.perms);
        draw_text(buf, win_w, win_h, x + 12.0, row + 16.0, &clip(&format!("Allowed: {bits}"), 48), 0xFF_9a_a4_b5);
    }
}

fn clip(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        text.to_string()
    } else {
        let t: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
