//! The viewer window (winit + softbuffer) and its network/decoder plumbing.
//!
//! ```text
//!  QUIC video stream ─► receiver task ─► decoder thread ─► latest frame ─► window (redraw)
//!                                             │ FrameAck
//!  window input events ───────────────────────┴──────────► sender task ─► QUIC control stream
//! ```
//!
//! Local shortcut (not sent to the host): Ctrl+Alt+Enter toggles full screen.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use pjdesk_proto::format_id;
use pjdesk_proto::peer::{ControlMsg, InputEvent, MouseButton, VideoHeader, KEY_NUMPAD_DIGIT};
use quinn::{Connection, SendStream};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CustomCursor, Fullscreen, Window, WindowId};

use super::decoder::VideoDecoder;
use super::render::{self, Layout, RgbFrame};
use super::toolbar::{self, Action, Combo};
use super::{keymap, Link};
use pjdesk_proto::peer::SpecialCommand;

/// Events delivered to the window from other threads.
#[derive(Debug)]
enum UserEvent {
    /// A new decoded frame is ready.
    Frame,
    /// New title text (statistics).
    Title(String),
    /// The connection ended.
    Closed(String),
}

/// Hand-off point between the decoder thread and the window.
#[derive(Default)]
struct FrameSlot {
    latest: Mutex<Option<RgbFrame>>,
    /// Pixel buffers the window is done with, reused by the decoder.
    spare: Mutex<Vec<Vec<u32>>>,
}

#[derive(Default)]
struct Stats {
    frames: AtomicU64,
    bytes: AtomicU64,
    encode_us: AtomicU64,
}

pub fn run(runtime: &Runtime, link: Link) -> Result<bool> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let slot = Arc::new(FrameSlot::default());
    let stats = Arc::new(Stats::default());
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlMsg>();
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<(VideoHeader, Vec<u8>)>();

    let Link { connection, endpoint, control_send, control_recv, host_name, host_id, path, peer_proto } = link;
    let title = format!("{host_name} ({}) — PJDesk, {path}", format_id(&host_id));

    runtime.spawn(send_control(control_send, control_rx));
    let wake_proxy = proxy.clone();
    let (live, cmds, recorder) = super::controls::start(
        connection.clone(),
        control_recv,
        control_tx.clone(),
        frame_tx,
        peer_proto,
        host_id.clone(),
        move || {
            let _ = wake_proxy.send_event(UserEvent::Frame);
        },
    );
    runtime.spawn(report_stats(connection.clone(), stats.clone(), proxy.clone(), title.clone()));
    {
        let (slot, control_tx, proxy, stats, recorder) =
            (slot.clone(), control_tx.clone(), proxy.clone(), stats.clone(), recorder.clone());
        std::thread::Builder::new().name("decoder".into()).spawn(move || {
            decode_loop(frame_rx, slot, control_tx, proxy, stats, recorder)
        })?;
    }

    let mut app = App {
        title,
        window: None,
        surface: None,
        slot,
        current: None,
        layout: Layout::default(),
        control: control_tx,
        sized_to_remote: false,
        keys_down: HashMap::new(),
        modifiers: ModifiersState::empty(),
        last_position: None,
        cursor_px: (0.0, 0.0),
        wheel: (0.0, 0.0),
        closed: false,
        toolbar: toolbar::Toolbar::default(),
        zoom: Zoom::Fit,
        pan: (0, 0),
        live,
        cmds,
        panel: super::panel::Panel::default(),
        restarting: false,
        peer_id: host_id,
    };
    if std::env::var("PJDESK_OPEN_PANEL").ok().as_deref() == Some("files") {
        app.panel.open = true;
        app.panel.tab = 0;
    }
    event_loop.run_app(&mut app)?;

    let restarting = app.restarting;
    if !restarting {
        connection.close(0u32.into(), b"viewer closed");
    }
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    });
    Ok(restarting)
}

/// Writes queued control messages, batching whatever is pending.
async fn send_control(mut send: SendStream, mut rx: mpsc::UnboundedReceiver<ControlMsg>) {
    let mut buf = Vec::new();
    while let Some(first) = rx.recv().await {
        buf.clear();
        let mut next = Some(first);
        while let Some(msg) = next {
            if let Ok(body) = postcard::to_stdvec(&msg) {
                buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
                buf.extend_from_slice(&body);
            }
            next = rx.try_recv().ok();
        }
        if send.write_all(&buf).await.is_err() {
            break;
        }
    }
}

async fn report_stats(connection: Connection, stats: Arc<Stats>, proxy: EventLoopProxy<UserEvent>, title: String) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let (mut frames_before, mut bytes_before) = (0u64, 0u64);
    loop {
        tick.tick().await;
        if connection.close_reason().is_some() {
            return;
        }
        let frames = stats.frames.load(Ordering::Relaxed);
        let bytes = stats.bytes.load(Ordering::Relaxed);
        let text = format!(
            "{title} — {} fps, {} kbit/s, {} ms RTT, {:.1} ms encode",
            frames - frames_before,
            (bytes - bytes_before) * 8 / 1000,
            connection.rtt().as_millis(),
            stats.encode_us.load(Ordering::Relaxed) as f64 / 1000.0
        );
        (frames_before, bytes_before) = (frames, bytes);
        if proxy.send_event(UserEvent::Title(text)).is_err() {
            return;
        }
    }
}

/// Decodes frames as they arrive and acknowledges each one (the host only
/// sends a new frame when one is acknowledged, which bounds latency).
fn decode_loop(
    frames: std::sync::mpsc::Receiver<(VideoHeader, Vec<u8>)>,
    slot: Arc<FrameSlot>,
    control: mpsc::UnboundedSender<ControlMsg>,
    proxy: EventLoopProxy<UserEvent>,
    stats: Arc<Stats>,
    recorder: Arc<Mutex<super::controls::RecorderSlot>>,
) {
    let mut decoder = match VideoDecoder::new() {
        Ok(decoder) => decoder,
        Err(e) => {
            let _ = proxy.send_event(UserEvent::Closed(format!("cannot start the H.264 decoder: {e:#}")));
            return;
        }
    };
    let mut waiting_for_keyframe = false;
    let mut last_request = Instant::now() - Duration::from_secs(10);

    while let Ok((header, data)) = frames.recv() {
        let finished = {
            let mut guard = recorder.lock().unwrap();
            if guard.push(&data, header.keyframe, header.width, header.height).is_err() {
                guard.stop()
            } else {
                None
            }
        };
        if let Some(done) = finished {
            let _ = done.finish();
        }
        stats.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        stats.encode_us.store(header.encode_us as u64, Ordering::Relaxed);
        if waiting_for_keyframe && !header.keyframe {
            let _ = control.send(ControlMsg::FrameAck { seq: header.seq });
            continue;
        }
        let mut pixels = slot.spare.lock().unwrap().pop().unwrap_or_default();
        match decoder.decode(&data, &mut pixels) {
            Ok(Some((width, height))) => {
                waiting_for_keyframe = false;
                *slot.latest.lock().unwrap() = Some(RgbFrame { width, height, pixels });
                stats.frames.fetch_add(1, Ordering::Relaxed);
                let _ = proxy.send_event(UserEvent::Frame);
            }
            Ok(None) => slot.spare.lock().unwrap().push(pixels),
            Err(e) => {
                warn!("Decoder error ({e:#}); asking the host for a keyframe");
                waiting_for_keyframe = true;
                slot.spare.lock().unwrap().push(pixels);
                if last_request.elapsed() > Duration::from_millis(500) {
                    last_request = Instant::now();
                    let _ = control.send(ControlMsg::RequestKeyframe);
                }
            }
        }
        let _ = control.send(ControlMsg::FrameAck { seq: header.seq });
    }
}

/// How the remote picture is scaled in the window.
#[derive(Clone, Copy)]
enum Zoom {
    /// Fit to the window, keeping aspect ratio (adaptive).
    Fit,
    /// An explicit scale (1.0 = pixel-for-pixel).
    Scale(f32),
}

struct App {
    title: String,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    slot: Arc<FrameSlot>,
    current: Option<RgbFrame>,
    /// Where the picture was last drawn (maps mouse positions).
    layout: Layout,
    control: mpsc::UnboundedSender<ControlMsg>,
    sized_to_remote: bool,
    /// Keys held down here, and the code each was sent as.
    keys_down: HashMap<KeyCode, u16>,
    modifiers: ModifiersState,
    last_position: Option<(u16, u16)>,
    /// Last pointer position in window pixels (for toolbar hit-testing).
    cursor_px: (f64, f64),
    /// Fractional wheel movement not sent yet (touchpads).
    wheel: (f64, f64),
    closed: bool,
    toolbar: super::toolbar::Toolbar,
    zoom: Zoom,
    /// Pan offset in window pixels when the picture is zoomed past the window.
    pan: (i32, i32),
    live: Arc<Mutex<super::controls::Live>>,
    cmds: mpsc::UnboundedSender<super::controls::Cmd>,
    panel: super::panel::Panel,
    restarting: bool,
    peer_id: String,
}

impl App {
    fn send(&self, event: InputEvent) {
        if !self.closed {
            let _ = self.control.send(ControlMsg::Input(event));
        }
    }

    fn release_all(&mut self) {
        self.keys_down.clear();
        self.send(InputEvent::ReleaseAll);
    }

    fn display_indexes(&self) -> Vec<u8> {
        let g = self.live.lock().unwrap();
        let mut v: Vec<u8> = if !g.displays.is_empty() {
            g.displays.iter().map(|d| d.index).collect()
        } else {
            g.monitors.iter().map(|m| m.index).collect()
        };
        v.sort_unstable();
        v.dedup();
        v
    }

    fn cycle_display(&mut self, next: bool) {
        let indexes = self.display_indexes();
        if indexes.is_empty() {
            return;
        }
        let current = self.live.lock().unwrap().current_display;
        let pos = indexes.iter().position(|i| *i == current).unwrap_or(0);
        let pos = if next { (pos + 1) % indexes.len() } else { (pos + indexes.len() - 1) % indexes.len() };
        self.choose_view(pjdesk_proto::peer::VIEW_SINGLE, indexes[pos]);
    }

    /// Switches display or view mode without reconnecting. `View` is sent only
    /// after the host has published a layout, so an older host only sees
    /// `SelectMonitor`.
    fn choose_view(&mut self, mode: u8, display: u8) {
        let known = {
            let mut g = self.live.lock().unwrap();
            let known = !g.displays.is_empty();
            if mode == pjdesk_proto::peer::VIEW_SINGLE {
                g.current_display = display;
            }
            if known {
                g.view_mode = mode;
            }
            known
        };
        if known {
            let _ = self.control.send(ControlMsg::View { mode, display });
            self.note_view(mode, display);
        }
        if mode == pjdesk_proto::peer::VIEW_SINGLE {
            let _ = self.control.send(ControlMsg::SelectMonitor { index: display });
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn note_view(&self, mode: u8, display: u8) {
        let id = self.peer_id.clone();
        if id.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            if let Ok(mut cfg) = crate::config::Config::load() {
                let note = cfg.settings.peers.entry(id).or_default();
                note.view_mode = mode;
                note.last_display = display;
                let _ = cfg.save();
            }
        });
    }

    fn redraw(&mut self) {
        if let Some(frame) = self.slot.latest.lock().unwrap().take() {
            if let Some(old) = self.current.replace(frame) {
                self.slot.spare.lock().unwrap().push(old.pixels);
            }
        }
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };

        // On the first frame, size the window to the remote screen (within ours).
        if !self.sized_to_remote {
            if let Some(frame) = &self.current {
                self.sized_to_remote = true;
                let monitor = window.current_monitor().map(|m| m.size()).unwrap_or(PhysicalSize::new(1920, 1080));
                let scale = (monitor.width as f64 * 0.9 / frame.width as f64)
                    .min(monitor.height as f64 * 0.85 / frame.height as f64)
                    .min(1.0);
                let _ = window.request_inner_size(PhysicalSize::new(
                    (frame.width as f64 * scale) as u32,
                    (frame.height as f64 * scale) as u32,
                ));
            }
        }

        let size = window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(width, height).is_err() {
            return;
        }
        let (win_w, win_h) = (size.width as usize, size.height as usize);
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        match &self.current {
            Some(frame) => {
                let layout = match self.zoom {
                    Zoom::Fit => render::fit(frame.width, frame.height, win_w, win_h),
                    Zoom::Scale(scale) => render::placement(frame.width, frame.height, win_w, win_h, scale, self.pan),
                };
                self.layout = layout;
                render::draw(frame, &mut buffer, win_w, win_h, layout);
            }
            None => buffer.fill(0x00_1e_1e_22),
        }
        let snap = self.live.lock().unwrap().snapshot();
        self.toolbar.sync(screens_of(&snap.displays), snap.current_display, snap.view_mode == pjdesk_proto::peer::VIEW_ALL);
        self.toolbar.draw(&mut buffer, win_w, win_h);
        self.panel.draw(&mut buffer, win_w, win_h, &snap);
        if snap.restart {
            self.restarting = true;
        }
        let _ = buffer.present();
    }

    /// Carries out a toolbar action. Returns true if the window should close.
    fn perform(&mut self, action: Action) -> bool {
        match action {
            Action::Fullscreen => {
                if let Some(window) = &self.window {
                    let on = window.fullscreen().is_none();
                    window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
                }
            }
            Action::ZoomFit => {
                self.zoom = Zoom::Fit;
                self.pan = (0, 0);
            }
            Action::ZoomTo(percent) => self.set_scale(percent as f32 / 100.0),
            Action::ZoomIn => self.set_scale(self.current_scale() * 1.25),
            Action::ZoomOut => self.set_scale(self.current_scale() / 1.25),
            Action::Keys(combo) => self.send_combo(combo),
            Action::Refresh => {
                let _ = self.control.send(ControlMsg::RequestKeyframe);
            }
            Action::ToggleBlockHost => self.toggle_host(pjdesk_proto::perms::BLOCK_INPUT, |on| ControlMsg::BlockInput { enabled: on }),
            Action::TogglePrivacy => self.toggle_host(pjdesk_proto::perms::PRIVACY, |on| ControlMsg::Privacy { enabled: on }),
            Action::ToggleAudio => self.toggle_host(pjdesk_proto::perms::AUDIO, |on| ControlMsg::Audio { enabled: on }),
            Action::ToggleRecord => {
                let on = !self.live.lock().unwrap().recording;
                let _ = self.cmds.send(super::controls::Cmd::Record(on));
            }
            Action::SendClipboard => {
                self.send_clip_text();
                let _ = self.cmds.send(super::controls::Cmd::ClipboardImage);
            }
            Action::Restart => {
                if !self.panel.confirm_restart {
                    self.panel.confirm_restart = true;
                    self.panel.open = true;
                    self.panel.tab = 2;
                    self.live.lock().unwrap().notices.push("Choose Restart again to restart the other computer.".into());
                } else if self.live.lock().unwrap().allowed(pjdesk_proto::perms::RESTART) {
                    self.panel.confirm_restart = false;
                    self.restarting = true;
                    let _ = self.control.send(ControlMsg::Restart);
                } else {
                    self.live.lock().unwrap().notices.push("The host did not allow a restart for this session.".into());
                }
            }
            Action::Quality(name) => {
                if let Some(q) = pjdesk_proto::quality::preset(name) {
                    if self.live.lock().unwrap().v3 {
                        let _ = self.control.send(ControlMsg::SetQuality(q));
                    }
                }
            }
            Action::OpenPanel(tab) => {
                if self.panel.open && self.panel.tab == tab {
                    self.panel.open = false;
                } else {
                    self.panel.open = true;
                    self.panel.tab = tab;
                    self.panel.focus = if tab == 1 { 1 } else { 0 };
                }
            }
            Action::Disconnect => return true,
            Action::SelectDisplay(index) => self.choose_view(pjdesk_proto::peer::VIEW_SINGLE, index),
            Action::ViewSingle => {
                let display = self.live.lock().unwrap().current_display;
                self.choose_view(pjdesk_proto::peer::VIEW_SINGLE, display);
            }
            Action::ViewAll => self.choose_view(pjdesk_proto::peer::VIEW_ALL, 0),
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        false
    }

    /// The scale currently in effect (the live fit scale when fitting).
    fn current_scale(&self) -> f32 {
        match self.zoom {
            Zoom::Scale(s) => s,
            Zoom::Fit => self.fit_scale(),
        }
    }

    /// The scale "fit to window" is using right now.
    fn fit_scale(&self) -> f32 {
        let (Some(window), Some(frame)) = (&self.window, &self.current) else { return 1.0 };
        let size = window.inner_size();
        (size.width as f32 / frame.width.max(1) as f32).min(size.height as f32 / frame.height.max(1) as f32)
    }

    fn set_scale(&mut self, scale: f32) {
        let new = scale.clamp(0.1, 8.0);
        let old = self.current_scale();
        let old_zoom = self.zoom;
        let old_pan = self.pan;
        // Keep the spot under the window centre fixed as the scale changes.
        if let (Some(window), Some((fw, fh))) =
            (self.window.clone(), self.current.as_ref().map(|f| (f.width, f.height)))
        {
            let size = window.inner_size();
            let (ww, wh) = (size.width as i32, size.height as i32);
            let old_layout = match old_zoom {
                Zoom::Fit => render::fit(fw, fh, ww as usize, wh as usize),
                Zoom::Scale(_) => render::placement(fw, fh, ww as usize, wh as usize, old, old_pan),
            };
            let cx = (ww / 2 - old_layout.x) as f32 / old_layout.width.max(1) as f32;
            let cy = (wh / 2 - old_layout.y) as f32 / old_layout.height.max(1) as f32;
            let niw = (fw as f32 * new) as i32;
            let nih = (fh as f32 * new) as i32;
            self.pan = (ww / 2 - (cx * niw as f32) as i32, wh / 2 - (cy * nih as f32) as i32);
        }
        self.zoom = Zoom::Scale(new);
    }

    /// Sends a key combination to the host: presses the keys in order, then
    /// releases them in reverse. Ctrl+Alt+Del goes through the host's secure
    /// path, since it can't be injected as plain keystrokes.
    fn toggle_host(&mut self, bit: u32, msg: impl FnOnce(bool) -> ControlMsg) {
        let mut g = self.live.lock().unwrap();
        if !g.v3 {
            g.notices.push("These controls need PJDesk 0.4 on both computers.".into());
            return;
        }
        if !g.allowed(bit) {
            g.notices.push("The host did not allow that for this session.".into());
            return;
        }
        let on = if bit == pjdesk_proto::perms::BLOCK_INPUT {
            g.block_host = !g.block_host;
            g.block_host
        } else if bit == pjdesk_proto::perms::PRIVACY {
            g.privacy = !g.privacy;
            g.privacy
        } else if bit == pjdesk_proto::perms::AUDIO {
            g.audio = !g.audio;
            g.audio
        } else {
            true
        };
        drop(g);
        let _ = self.control.send(msg(on));
    }

    fn send_clip_text(&mut self) {
        let mut g = self.live.lock().unwrap();
        if !g.v3 {
            g.notices.push("Clipboard sync needs PJDesk 0.4 on both computers.".into());
            return;
        }
        if !g.allowed(pjdesk_proto::perms::CLIPBOARD) {
            g.notices.push("Clipboard sync was not allowed for this session.".into());
            return;
        }
        drop(g);
        let Some(text) = crate::clip::get_text() else { return };
        if text.len() > pjdesk_proto::peer::MAX_CLIP_TEXT {
            self.live.lock().unwrap().notices.push("That clipboard text is too long.".into());
            return;
        }
        self.live.lock().unwrap().remember_clip(&text);
        self.live.lock().unwrap().notices.push("Sent the clipboard.".into());
        let byte_len = text.len() as u32;
        let _ = self.control.send(ControlMsg::Clipboard(pjdesk_proto::peer::Clipboard {
            kind: pjdesk_proto::peer::ClipKind::Text,
            bytes: text.into_bytes(),
            byte_len,
        }));
    }

    fn apply_panel(&mut self, cmd: super::panel::PanelCmd) {
        use super::controls::Cmd;
        use super::panel::PanelCmd;
        match cmd {
            PanelCmd::SendFile(path) => {
                let _ = self.cmds.send(Cmd::SendFile(std::path::PathBuf::from(path)));
            }
            PanelCmd::Accept(id) => {
                let _ = self.cmds.send(Cmd::Accept(id));
            }
            PanelCmd::Pull(name) => {
                let _ = self.cmds.send(Cmd::Pull(name));
            }
            PanelCmd::List => {
                let _ = self.cmds.send(Cmd::ListRemote);
            }
            PanelCmd::Chat(text) => {
                let mut g = self.live.lock().unwrap();
                if !g.v3 {
                    g.notices.push("Chat needs PJDesk 0.4 on both computers.".into());
                } else if !g.allowed(pjdesk_proto::perms::CHAT) {
                    g.notices.push("Chat was not allowed for this session.".into());
                } else {
                    g.chat.push((true, text.clone()));
                    drop(g);
                    let _ = self.control.send(ControlMsg::Chat { text });
                }
            }
            PanelCmd::Tunnel { local, host, port } => {
                let _ = self.cmds.send(Cmd::Tunnel { local, host, port });
            }
            PanelCmd::Monitor(index) => {
                if self.live.lock().unwrap().v3 {
                    let _ = self.control.send(ControlMsg::SelectMonitor { index });
                }
            }
        }
    }

    fn send_combo(&mut self, combo: Combo) {
        if combo == Combo::CtrlAltDel {
            let _ = self.control.send(ControlMsg::Special(SpecialCommand::CtrlAltDel));
            return;
        }
        // PC/AT set-1 scan codes; 0xE0__ marks an extended key.
        const LCTRL: u16 = 0x1D;
        const LALT: u16 = 0x38;
        const LSHIFT: u16 = 0x2A;
        const ESC: u16 = 0x01;
        const TAB: u16 = 0x0F;
        const F4: u16 = 0x3E;
        const D: u16 = 0x20;
        const E: u16 = 0x12;
        const L: u16 = 0x26;
        const LWIN: u16 = 0xE05B;
        const PRTSC: u16 = 0xE037;
        let keys: &[u16] = match combo {
            Combo::Win => &[LWIN],
            Combo::AltTab => &[LALT, TAB],
            Combo::CtrlEsc => &[LCTRL, ESC],
            Combo::WinD => &[LWIN, D],
            Combo::WinE => &[LWIN, E],
            Combo::WinL => &[LWIN, L],
            Combo::TaskMgr => &[LCTRL, LSHIFT, ESC],
            Combo::AltF4 => &[LALT, F4],
            Combo::PrintScreen => &[PRTSC],
            Combo::CtrlAltDel => return,
        };
        // Clear anything held locally so modifiers don't pile up on the host.
        self.release_all();
        for &scan in keys {
            self.send(InputEvent::Key { scan, down: true });
        }
        for &scan in keys.iter().rev() {
            self.send(InputEvent::Key { scan, down: false });
        }
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let down = event.state == ElementState::Pressed;

        if self.panel.focus != 0 && down {
            if code == KeyCode::Escape {
                self.panel.focus = 0;
                return;
            }
            if code == KeyCode::Backspace {
                self.panel.backspace();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            if code == KeyCode::Enter && self.panel.focus == 1 {
                if let Some(cmd) = self.panel.submit_chat() {
                    self.apply_panel(cmd);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            if let Some(text) = &event.text {
                let text = text.to_string();
                self.panel.type_text(&text);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
        }
        if down && !event.repeat && self.modifiers.control_key() && self.modifiers.alt_key() {
            if let Some(n) = display_key(code) {
                if let Some(index) = self.display_indexes().get(n).copied() {
                    self.choose_view(pjdesk_proto::peer::VIEW_SINGLE, index);
                }
                return;
            }
            if matches!(code, KeyCode::ArrowLeft | KeyCode::ArrowRight) {
                self.cycle_display(code == KeyCode::ArrowRight);
                return;
            }
        }
        if down && code == KeyCode::Enter && self.modifiers.control_key() && self.modifiers.alt_key() {
            if let Some(window) = &self.window {
                let fullscreen = window.fullscreen().is_none();
                window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
            }
            let _ = event_loop;
            return;
        }
        // "Block input" also suppresses forwarded keystrokes.
        if self.toolbar.block_input {
            return;
        }
        let Some(mut scan) = keymap::scancode(code) else { return };
        if down {
            // Numpad keys typed as digits here (NumLock on) are sent as digits,
            // so they don't turn into arrow keys when the host's NumLock is off.
            if keymap::is_numpad_digit_key(code) && matches!(event.logical_key, Key::Character(_)) {
                scan |= KEY_NUMPAD_DIGIT;
            }
            self.keys_down.insert(code, scan);
        } else if let Some(pressed_as) = self.keys_down.remove(&code) {
            // Release exactly what was pressed.
            scan = pressed_as;
        }
        self.send(InputEvent::Key { scan, down });
    }

    fn on_cursor(&mut self, position: PhysicalPosition<f64>) {
        self.cursor_px = (position.x, position.y);
        // The toolbar reveals near the top; hovering it must not move the
        // remote pointer.
        if let Some(window) = self.window.clone() {
            let win_w = window.inner_size().width as usize;
            let was_visible = self.toolbar.is_visible();
            let over_panel = self.panel.covers(position.x, position.y, win_w, window.inner_size().height as usize);
            let over_bar = self.toolbar.on_pointer(position.x, position.y, win_w);
            if over_panel {
                window.request_redraw();
                return;
            }
            if self.toolbar.is_visible() || was_visible {
                window.request_redraw();
            }
            if over_bar {
                return;
            }
        }
        // When zoomed in past the window, pan to keep the pointer in sight: a
        // dead-zone near each edge nudges the picture along.
        if let (Zoom::Scale(_), Some(window)) = (self.zoom, self.window.clone()) {
            let size = window.inner_size();
            let (ww, wh) = (size.width as i32, size.height as i32);
            let (iw, ih) = (self.layout.width as i32, self.layout.height as i32);
            let mut moved = false;
            if iw > ww {
                let margin = ww / 6;
                let cx = position.x as i32;
                if cx < margin {
                    self.pan.0 += margin - cx;
                    moved = true;
                } else if cx > ww - margin {
                    self.pan.0 -= cx - (ww - margin);
                    moved = true;
                }
                self.pan.0 = self.pan.0.clamp(ww - iw, 0);
            }
            if ih > wh {
                let margin = wh / 6;
                let cy = position.y as i32;
                if cy < margin {
                    self.pan.1 += margin - cy;
                    moved = true;
                } else if cy > wh - margin {
                    self.pan.1 -= cy - (wh - margin);
                    moved = true;
                }
                self.pan.1 = self.pan.1.clamp(wh - ih, 0);
            }
            if moved {
                window.request_redraw();
            }
        }
        if self.toolbar.block_input {
            return;
        }
        let layout = self.layout;
        if layout.width == 0 || layout.height == 0 {
            return;
        }
        // Pixel centers: the first and last pixel map to 0 and 65535.
        let fx = ((position.x - layout.x as f64) / (layout.width.max(2) - 1) as f64).clamp(0.0, 1.0);
        let fy = ((position.y - layout.y as f64) / (layout.height.max(2) - 1) as f64).clamp(0.0, 1.0);
        let point = ((fx * 65535.0).round() as u16, (fy * 65535.0).round() as u16);
        if self.last_position != Some(point) {
            self.last_position = Some(point);
            self.send(InputEvent::MouseMove { x: point.0, y: point.1 });
        }
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        // Windows wheel units: 120 per notch, positive = away from the user / right.
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64 * 120.0, y as f64 * 120.0),
            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
        };
        self.wheel.0 -= x; // winit: positive x scrolls content right (= wheel tilted left)
        self.wheel.1 += y;
        let dx = self.wheel.0.trunc();
        let dy = self.wheel.1.trunc();
        self.wheel.0 -= dx;
        self.wheel.1 -= dy;
        if dx != 0.0 || dy != 0.0 {
            self.send(InputEvent::Wheel {
                dx: dx.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
                dy: dy.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
            });
        }
    }
}

/// A small dot used as the local cursor; the host's real cursor is drawn
/// into the video, so this just marks the exact click point without hiding it.
fn dot_cursor() -> Vec<u8> {
    const SIZE: usize = 5;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as i32 - 2, y as i32 - 2);
            let d2 = dx * dx + dy * dy;
            let px = if d2 <= 1 {
                [255, 255, 255, 255]
            } else if d2 <= 5 {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[(y * SIZE + x) * 4..(y * SIZE + x) * 4 + 4].copy_from_slice(&px);
        }
    }
    rgba
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(PhysicalSize::new(1280u32, 720u32));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(e) => {
                eprintln!("Cannot open a window: {e}");
                event_loop.exit();
                return;
            }
        };
        if let Ok(source) = CustomCursor::from_rgba(dot_cursor(), 5, 5, 2, 2) {
            window.set_cursor(event_loop.create_custom_cursor(source));
        }
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        match surface {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => {
                eprintln!("Cannot draw into the window: {e}");
                event_loop.exit();
                return;
            }
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(window) = self.window.clone() else { return };
        match event {
            UserEvent::Frame => {
                if self.live.lock().unwrap().restart {
                    self.restarting = true;
                    _event_loop.exit();
                    return;
                }
                window.request_redraw();
            }
            UserEvent::Title(text) => {
                if !self.closed {
                    window.set_title(&text);
                }
            }
            UserEvent::Closed(reason) => {
                self.closed = true;
                info!("Disconnected: {reason}");
                window.set_title(&format!("{} — DISCONNECTED: {reason}", self.title));
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            // Don't leave keys "held" on the host when we lose focus.
            WindowEvent::Focused(false) => self.release_all(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => self.on_key(event_loop, event),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor(position),
            WindowEvent::MouseInput { state, button, .. } => {
                // Clicks on the toolbar (or its open menu) are handled locally.
                if button == winit::event::MouseButton::Left {
                    let win_w = self.window.as_ref().map(|w| w.inner_size().width as usize).unwrap_or(0);
                    let win_h = self.window.as_ref().map(|w| w.inner_size().height as usize).unwrap_or(0);
                    if self.panel.covers(self.cursor_px.0, self.cursor_px.1, win_w, win_h) {
                        if state == ElementState::Pressed {
                            let snap = self.live.lock().unwrap().snapshot();
                            if let Some(cmd) = self.panel.click(self.cursor_px.0, self.cursor_px.1, win_w, win_h, &snap) {
                                self.apply_panel(cmd);
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        return;
                    }
                    if self.toolbar.covers(self.cursor_px.0, self.cursor_px.1, win_w) {
                        if state == ElementState::Pressed {
                            if let Some(action) = self.toolbar.click(self.cursor_px.0, self.cursor_px.1, win_w) {
                                if self.perform(action) {
                                    event_loop.exit();
                                }
                            } else if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        return;
                    }
                }
                if self.toolbar.block_input {
                    return;
                }
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    winit::event::MouseButton::Back => MouseButton::Back,
                    winit::event::MouseButton::Forward => MouseButton::Forward,
                    winit::event::MouseButton::Other(_) => return,
                };
                self.send(InputEvent::MouseButton { button, down: state == ElementState::Pressed });
            }
            WindowEvent::DroppedFile(path) => {
                let _ = self.cmds.send(super::controls::Cmd::SendFile(path));
                self.panel.open = true;
                self.panel.tab = 0;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.modifiers.control_key() {
                    // Ctrl + wheel zooms the view locally instead of scrolling
                    // the remote.
                    let up = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y > 0.0,
                        MouseScrollDelta::PixelDelta(p) => p.y > 0.0,
                    };
                    self.perform(if up { Action::ZoomIn } else { Action::ZoomOut });
                } else if !self.toolbar.block_input {
                    self.on_wheel(delta);
                }
            }
            _ => {}
        }
    }
}

fn screens_of(displays: &[pjdesk_proto::peer::DisplayLayout]) -> Vec<pjdesk_proto::layout::Screen> {
    displays
        .iter()
        .map(|d| pjdesk_proto::layout::Screen {
            index: d.index,
            x: d.x,
            y: d.y,
            width: d.width as u32,
            height: d.height as u32,
            dpi: d.dpi,
        })
        .collect()
}

fn display_key(code: KeyCode) -> Option<usize> {
    Some(match code {
        KeyCode::Digit1 | KeyCode::Numpad1 => 0,
        KeyCode::Digit2 | KeyCode::Numpad2 => 1,
        KeyCode::Digit3 | KeyCode::Numpad3 => 2,
        KeyCode::Digit4 | KeyCode::Numpad4 => 3,
        KeyCode::Digit5 | KeyCode::Numpad5 => 4,
        KeyCode::Digit6 | KeyCode::Numpad6 => 5,
        KeyCode::Digit7 | KeyCode::Numpad7 => 6,
        KeyCode::Digit8 | KeyCode::Numpad8 => 7,
        KeyCode::Digit9 | KeyCode::Numpad9 => 8,
        _ => return None,
    })
}
