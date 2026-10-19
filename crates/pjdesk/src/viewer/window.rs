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
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decode_loop(frame_rx, slot, control_tx, proxy, stats, recorder))?;
    }

    let loaded = crate::config::Config::load().ok();
    let theme_name = loaded.as_ref().map(|c| c.settings.theme.clone()).unwrap_or_default();
    let mut toolbar = toolbar::Toolbar::default();
    let follow_system_theme = theme_name != "light" && theme_name != "dark";
    if theme_name == "light" {
        toolbar.set_light(true);
    }
    let places =
        loaded.as_ref().and_then(|c| c.settings.peers.get(&host_id).map(|n| n.windows.clone())).unwrap_or_default();
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
        toolbar,
        zoom: Zoom::Fit,
        pan: (0, 0),
        live,
        cmds,
        panel: super::panel::Panel::default(),
        restarting: false,
        peer_id: host_id.clone(),
        host_name,
        extra: Vec::new(),
        primary_display: std::env::var("PJDESK_DISPLAY").ok().and_then(|s| s.parse().ok()).unwrap_or(0),
        toast: None,
        sent_watch: Vec::new(),
        places,
        place_saved: Instant::now() - Duration::from_secs(5),
        placed_primary: false,
        follow_system_theme,
        noted_platform: String::new(),
        remote_size: None,
        android_stroke: pjdesk_proto::gesture::StrokeTracker::default(),
        pointer_down: false,
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
    host_name: String,
    extra: Vec<ExtraPane>,
    primary_display: u8,
    toast: Option<(String, Instant)>,
    sent_watch: Vec<u8>,
    places: Vec<crate::config::SavedWindow>,
    place_saved: Instant,
    placed_primary: bool,
    /// Settings left the theme on "system", so the window follows the OS.
    follow_system_theme: bool,
    noted_platform: String,
    remote_size: Option<(usize, usize)>,
    /// Press on an Android picture, turned into a tap or a swipe on release.
    android_stroke: pjdesk_proto::gesture::StrokeTracker,
    pointer_down: bool,
}

struct ExtraPane {
    display: u8,
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    layout: Layout,
    cursor: (f64, f64),
    /// True once a saved place or the first picture has set the window size.
    sized: bool,
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
        self.pump_frame();
        let separate = self.is_separate();
        let display = self.primary_display;
        let cropped = if separate { self.tile_picture(display) } else { None };
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
        let snap = self.live.lock().unwrap().snapshot();
        self.toolbar.sync(screens_of(&snap.displays), snap.current_display, snap.view_mode);
        self.toolbar.set_android(snap.platform_family == "android");
        if !snap.platform_family.is_empty() && snap.platform_family != self.noted_platform {
            self.noted_platform = snap.platform_family.clone();
            let (id, family, version) =
                (self.peer_id.clone(), snap.platform_family.clone(), snap.platform_version.clone());
            std::thread::spawn(move || crate::config::Config::remember_platform(&id, &family, &version));
        }

        // Size the window to the picture, and again when it rotates or the
        // selected display has a different size.
        if let Some(frame) = if separate { cropped.as_ref() } else { self.current.as_ref() } {
            let size = (frame.width, frame.height);
            if self.remote_size != Some(size) {
                self.remote_size = Some(size);
                self.sized_to_remote = true;
                let extra = self.toolbar.tab_height() as u32;
                Self::fit_remote(window, frame.width, frame.height, extra);
            }
        }

        let size = window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(width, height).is_err() {
            return;
        }
        let (win_w, win_h) = (size.width as usize, size.height as usize);
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        let shown = if separate { cropped.as_ref() } else { self.current.as_ref() };
        let origin = self.toolbar.tab_height() as usize;
        let layout = Self::paint_picture(
            &mut buffer,
            win_w,
            win_h,
            shown,
            self.zoom,
            self.pan,
            origin,
            self.toolbar.letterbox(),
        );
        self.layout = layout;
        if separate && cropped.is_none() {
            toolbar::draw_text(
                &mut buffer,
                win_w,
                win_h,
                24.0,
                origin as f32 + 48.0,
                "Waiting for this screen",
                0xFF_9a_a3_b5,
            );
        }
        Self::paint_overlay(&self.toolbar, &self.toast, &mut buffer, win_w, win_h, display, separate, true);
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
                    Self::toggle_fullscreen(window);
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
            Action::ToggleBlockHost => {
                self.toggle_host(pjdesk_proto::perms::BLOCK_INPUT, |on| ControlMsg::BlockInput { enabled: on })
            }
            Action::TogglePrivacy => {
                self.toggle_host(pjdesk_proto::perms::PRIVACY, |on| ControlMsg::Privacy { enabled: on })
            }
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
                    self.live
                        .lock()
                        .unwrap()
                        .notices
                        .push("Choose Restart again to restart the other computer.".into());
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
            Action::ViewSeparate => {
                let display = self.live.lock().unwrap().current_display;
                self.primary_display = display;
                self.sent_watch.clear();
                self.placed_primary = false;
                self.choose_view(pjdesk_proto::peer::VIEW_SEPARATE, display);
            }
            Action::CloseScreen => return self.close_primary_screen(),
            Action::Android(which) => self.send_android(which),
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
        let (Some(_window), Some(frame)) = (&self.window, &self.current) else { return 1.0 };
        let (ww, wh, _) = self.content_extent();
        (ww as f32 / frame.width.max(1) as f32).min(wh as f32 / frame.height.max(1) as f32)
    }

    /// Picture area below the tab strip: width, height, and the strip's height.
    fn content_extent(&self) -> (i32, i32, i32) {
        let Some(window) = &self.window else { return (1, 1, 0) };
        let size = window.inner_size();
        let origin = self.toolbar.tab_height() as i32;
        (size.width as i32, (size.height as i32 - origin).max(1), origin)
    }

    fn set_scale(&mut self, scale: f32) {
        let new = scale.clamp(0.1, 8.0);
        let old = self.current_scale();
        let old_zoom = self.zoom;
        let old_pan = self.pan;
        // Keep the spot under the window centre fixed as the scale changes.
        if let (Some(_window), Some((fw, fh))) =
            (self.window.clone(), self.current.as_ref().map(|f| (f.width, f.height)))
        {
            let (ww, wh, _) = self.content_extent();
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

    fn on_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent, focused: Option<Rc<Window>>) {
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
        if down && !event.repeat && code == KeyCode::F11 {
            let window = focused.clone().or_else(|| self.window.clone());
            if let Some(window) = window {
                Self::toggle_fullscreen(&window);
            }
            return;
        }
        if down && !event.repeat && self.modifiers.control_key() && self.modifiers.alt_key() {
            if let Some(n) = display_key(code) {
                if let Some(index) = self.display_indexes().get(n).copied() {
                    if self.is_separate() {
                        self.focus_display(index);
                    } else {
                        self.choose_view(pjdesk_proto::peer::VIEW_SINGLE, index);
                    }
                }
                return;
            }
            if matches!(code, KeyCode::ArrowLeft | KeyCode::ArrowRight) {
                self.cycle_display(code == KeyCode::ArrowRight);
                return;
            }
        }
        if down && code == KeyCode::Enter && self.modifiers.control_key() && self.modifiers.alt_key() {
            let window = focused.or_else(|| self.window.clone());
            if let Some(window) = window {
                Self::toggle_fullscreen(&window);
            }
            let _ = event_loop;
            return;
        }
        // "Block input" also suppresses forwarded keystrokes.
        if self.toolbar.block_input {
            return;
        }
        if self.remote_is_android() {
            self.send_android_key(&event, code, down);
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
            if self.is_separate()
                && toolbar::compact_covers(position.x, position.y, win_w, window.inner_size().height as usize)
            {
                window.request_redraw();
                return;
            }
        }
        if self.is_separate() {
            if self.toolbar.block_input {
                return;
            }
            let Some(window) = self.window.clone() else { return };
            let size = window.inner_size();
            if let Some((x, y)) =
                self.pointer_for(self.primary_display, position, size.width as f64, size.height as f64)
            {
                self.send_pointer(self.primary_display, x, y);
            }
            return;
        }
        // When zoomed in past the window, pan to keep the pointer in sight: a
        // dead-zone near each edge nudges the picture along.
        if let (Zoom::Scale(_), Some(window)) = (self.zoom, self.window.clone()) {
            let _ = window;
            let (ww, wh, origin) = self.content_extent();
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
                let cy = position.y as i32 - origin;
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
        if self.remote_is_android() {
            // A click or drag is one gesture, sent when the button comes up.
            // Moving the mouse with the button up does not hover the phone.
            if self.pointer_down {
                self.android_stroke.move_to(point.0, point.1);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Some(gesture) = self.android_stroke.poll_long_press(now) {
                    self.send(gesture.to_event());
                }
            }
            self.last_position = Some(point);
            return;
        }
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
            if self.remote_is_android() {
                let (x, y) = self.last_position.unwrap_or((32768, 32768));
                let notches = (dy / 120.0).round() as i32;
                if notches != 0 {
                    self.send(pjdesk_proto::gesture::wheel_swipe(x, y, notches).to_event());
                }
            } else {
                self.send(InputEvent::Wheel {
                    dx: dx.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
                    dy: dy.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
                });
            }
        }
    }

    fn remote_is_android(&self) -> bool {
        self.live.lock().unwrap().platform_family == "android"
    }

    fn on_android_button(&mut self, down: bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if down {
            let Some(point) = self.pointer_norm() else { return };
            self.pointer_down = true;
            self.android_stroke.down(now, point.0, point.1);
            self.last_position = Some(point);
        } else if self.pointer_down {
            self.pointer_down = false;
            if let Some(gesture) = self.android_stroke.up(now) {
                self.send(gesture.to_event());
            }
        }
    }

    fn send_android(&mut self, which: u8) {
        use pjdesk_proto::peer::SpecialCommand;
        let cmd = match which {
            0 => SpecialCommand::Back,
            1 => SpecialCommand::Home,
            2 => SpecialCommand::Recents,
            3 => SpecialCommand::VolumeUp,
            4 => SpecialCommand::VolumeDown,
            _ => SpecialCommand::Power,
        };
        let _ = self.control.send(ControlMsg::Special(cmd));
    }

    fn pointer_norm(&self) -> Option<(u16, u16)> {
        let layout = self.layout;
        if layout.width < 2 || layout.height < 2 {
            return None;
        }
        let placed = pjdesk_proto::gesture::Letterbox {
            x: layout.x,
            y: layout.y,
            width: layout.width as u32,
            height: layout.height as u32,
        };
        pjdesk_proto::gesture::view_to_norm(placed, self.cursor_px.0 as f32, self.cursor_px.1 as f32)
    }

    /// Android has no scan-code keyboard. Printable keys and Enter/Backspace
    /// go as text; the host types them into the focused field.
    fn send_android_key(&mut self, event: &KeyEvent, code: KeyCode, down: bool) {
        if !down {
            return;
        }
        if code == KeyCode::Backspace {
            let _ = self.control.send(ControlMsg::Text { chars: "\u{8}".into() });
            return;
        }
        if code == KeyCode::Enter {
            let _ = self.control.send(ControlMsg::Text { chars: "\n".into() });
            return;
        }
        if let Some(text) = &event.text {
            let text = text.to_string();
            if !text.is_empty() && !text.chars().all(|c| c.is_control()) {
                let _ = self.control.send(ControlMsg::Text { chars: text });
            }
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

impl App {
    fn pump_frame(&mut self) {
        if let Some(frame) = self.slot.latest.lock().unwrap().take() {
            if let Some(old) = self.current.replace(frame) {
                self.slot.spare.lock().unwrap().push(old.pixels);
            }
        }
    }

    fn is_separate(&self) -> bool {
        self.live.lock().unwrap().view_mode == pjdesk_proto::peer::VIEW_SEPARATE
    }

    fn screen_named(&self, index: u8) -> String {
        format!("{} — Screen {}", self.host_name, index + 1)
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    fn tile_picture(&self, display: u8) -> Option<super::render::RgbFrame> {
        let (aw, ah, tile) = {
            let g = self.live.lock().unwrap();
            if g.view_mode != pjdesk_proto::peer::VIEW_SEPARATE {
                return None;
            }
            let tile = g.atlas.iter().copied().find(|t| t.index == display)?;
            (g.atlas_w, g.atlas_h, tile)
        };
        let frame = self.current.as_ref()?;
        super::render::crop_tile(frame, aw, ah, tile.x, tile.y, tile.width, tile.height)
    }

    fn paint_picture(
        buffer: &mut [u32],
        win_w: usize,
        win_h: usize,
        frame: Option<&super::render::RgbFrame>,
        zoom: Zoom,
        pan: (i32, i32),
        origin_y: usize,
        background: u32,
    ) -> Layout {
        let content_h = win_h.saturating_sub(origin_y).max(1);
        match frame {
            Some(frame) => {
                let mut layout = match zoom {
                    Zoom::Fit => render::fit(frame.width, frame.height, win_w, content_h),
                    Zoom::Scale(scale) => render::placement(frame.width, frame.height, win_w, content_h, scale, pan),
                };
                layout.y += origin_y as i32;
                render::draw_on(frame, buffer, win_w, win_h, layout, background);
                layout
            }
            None => {
                buffer.fill(background);
                Layout::default()
            }
        }
    }

    fn paint_overlay(
        toolbar: &super::toolbar::Toolbar,
        toast: &Option<(String, Instant)>,
        buffer: &mut [u32],
        win_w: usize,
        win_h: usize,
        display: u8,
        separate: bool,
        full_toolbar: bool,
    ) {
        if full_toolbar {
            toolbar.draw(buffer, win_w, win_h);
            toolbar.draw_tabs(buffer, win_w, win_h);
        }
        if separate {
            toolbar::draw_compact(buffer, win_w, win_h, &format!("Screen {}", display + 1));
        }
        if let Some((text, at)) = toast {
            if at.elapsed() < Duration::from_secs(4) {
                toolbar::draw_toast(buffer, win_w, win_h, text);
            }
        }
    }

    fn pointer_for(&self, display: u8, position: PhysicalPosition<f64>, win_w: f64, win_h: f64) -> Option<(u16, u16)> {
        let screen = {
            let g = self.live.lock().unwrap();
            screens_of(&g.displays).into_iter().find(|s| s.index == display)?
        };
        let origin = self.toolbar.tab_height() as f32;
        let content_h = (win_h as f32 - origin).max(1.0);
        pjdesk_proto::layout::window_to_pointer(
            screen,
            position.x as f32,
            position.y as f32 - origin,
            win_w as f32,
            content_h,
        )
        .map(|(_, _, nx, ny)| (nx, ny))
    }

    fn send_pointer(&self, display: u8, x: u16, y: u16) {
        let _ = self.control.send(ControlMsg::Pointer { display, x, y });
    }

    fn toggle_fullscreen(window: &Window) {
        let on = window.fullscreen().is_none();
        let target = on.then(|| Fullscreen::Borderless(window.current_monitor()));
        window.set_fullscreen(target);
    }

    /// Restores a saved place when that local monitor is still attached.
    /// Returns true when a saved size was applied, so the first picture does
    /// not overwrite it.
    fn apply_place(&self, event_loop: &ActiveEventLoop, window: &Window, display: u8) -> bool {
        let Some(place) = self.places.iter().find(|p| p.display == display && !p.monitor.is_empty()) else {
            return false;
        };
        let present = event_loop.available_monitors().filter_map(|m| m.name()).any(|n| n == place.monitor);
        if !present {
            return false;
        }
        let sized = place.width >= 320 && place.height >= 200;
        if sized {
            let _ = window.request_inner_size(PhysicalSize::new(place.width, place.height));
        }
        window.set_outer_position(PhysicalPosition::new(place.x, place.y));
        sized
    }

    fn fit_remote(window: &Window, frame_w: usize, frame_h: usize, extra_h: u32) {
        if frame_w == 0 || frame_h == 0 {
            return;
        }
        let monitor = window.current_monitor().map(|m| m.size()).unwrap_or(PhysicalSize::new(1920, 1080));
        let scale =
            (monitor.width as f64 * 0.9 / frame_w as f64).min(monitor.height as f64 * 0.85 / frame_h as f64).min(1.0);
        let _ = window.request_inner_size(PhysicalSize::new(
            ((frame_w as f64 * scale) as u32).max(320),
            ((frame_h as f64 * scale) as u32).max(200) + extra_h,
        ));
    }

    fn place_of(window: &Window, display: u8) -> Option<crate::config::SavedWindow> {
        let pos = window.outer_position().ok()?;
        let size = window.inner_size();
        Some(crate::config::SavedWindow {
            display,
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
            monitor: window.current_monitor().and_then(|m| m.name()).unwrap_or_default(),
        })
    }

    fn remember_places(&mut self, force: bool) {
        if self.peer_id.is_empty() || !self.is_separate() {
            return;
        }
        if !force && self.place_saved.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.place_saved = Instant::now();
        let mut windows = Vec::new();
        if let Some(window) = &self.window {
            if let Some(place) = Self::place_of(window, self.primary_display) {
                windows.push(place);
            }
        }
        for pane in &self.extra {
            if let Some(place) = Self::place_of(&pane.window, pane.display) {
                windows.push(place);
            }
        }
        self.places = windows.clone();
        let id = self.peer_id.clone();
        let mode = self.live.lock().unwrap().view_mode;
        let display = self.primary_display;
        std::thread::spawn(move || {
            if let Ok(mut cfg) = crate::config::Config::load() {
                let note = cfg.settings.peers.entry(id).or_default();
                note.view_mode = mode;
                note.last_display = display;
                note.windows = windows;
                let _ = cfg.save();
            }
        });
    }

    fn push_watch(&mut self) {
        if !self.is_separate() {
            return;
        }
        let mut displays = vec![self.primary_display];
        displays.extend(self.extra.iter().map(|p| p.display));
        displays.sort_unstable();
        displays.dedup();
        if displays == self.sent_watch {
            return;
        }
        self.sent_watch = displays.clone();
        let _ = self.control.send(ControlMsg::Watch { displays });
    }

    fn open_extra(&mut self, event_loop: &ActiveEventLoop, display: u8) {
        let attributes = Window::default_attributes()
            .with_title(self.screen_named(display))
            .with_inner_size(PhysicalSize::new(960u32, 600u32));
        let Ok(window) = event_loop.create_window(attributes) else { return };
        let window = Rc::new(window);
        let sized = self.apply_place(event_loop, &window, display);
        if let Ok(source) = CustomCursor::from_rgba(dot_cursor(), 5, 5, 2, 2) {
            window.set_cursor(event_loop.create_custom_cursor(source));
        }
        let Ok(context) = softbuffer::Context::new(window.clone()) else { return };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else { return };
        window.request_redraw();
        self.extra.push(ExtraPane { display, window, surface, layout: Layout::default(), cursor: (0.0, 0.0), sized });
    }

    fn promote(&mut self, pane: ExtraPane) {
        self.primary_display = pane.display;
        pane.window.set_title(&self.screen_named(pane.display));
        self.window = Some(pane.window);
        self.surface = Some(pane.surface);
        self.sized_to_remote = pane.sized;
    }

    /// Closes the primary screen window. Returns true when that was the last
    /// window and the session should end.
    fn close_primary_screen(&mut self) -> bool {
        self.remember_places(true);
        if self.extra.is_empty() {
            return true;
        }
        let pane = self.extra.remove(0);
        let closed = self.primary_display;
        self.promote(pane);
        self.push_watch();
        self.show_toast(format!("Screen {} closed", closed + 1));
        false
    }

    fn sync_separate(&mut self, event_loop: &ActiveEventLoop) {
        let (mode, displays) = {
            let g = self.live.lock().unwrap();
            (g.view_mode, g.displays.clone())
        };
        if mode != pjdesk_proto::peer::VIEW_SEPARATE {
            if !self.extra.is_empty() || self.placed_primary {
                self.extra.clear();
                self.sent_watch.clear();
                self.placed_primary = false;
                if let Some(window) = &self.window {
                    window.set_title(&self.title);
                    window.request_redraw();
                }
            }
            return;
        }
        let indexes: Vec<u8> = displays.iter().map(|d| d.index).collect();
        if indexes.is_empty() {
            return;
        }
        if self.sent_watch.is_empty() {
            let current = self.live.lock().unwrap().current_display;
            if indexes.contains(&current) {
                self.primary_display = current;
            } else if !indexes.contains(&self.primary_display) {
                self.primary_display = indexes[0];
            }
        }
        if !indexes.contains(&self.primary_display) {
            let gone = self.primary_display;
            self.show_toast(format!("Screen {} was disconnected", gone + 1));
            if let Some(pos) = self.extra.iter().position(|p| indexes.contains(&p.display)) {
                let pane = self.extra.remove(pos);
                self.promote(pane);
            } else {
                self.primary_display = indexes[0];
            }
        }
        let mut gone = Vec::new();
        self.extra.retain(|p| {
            if indexes.contains(&p.display) {
                true
            } else {
                gone.push(p.display);
                false
            }
        });
        for display in gone {
            self.show_toast(format!("Screen {} was disconnected", display + 1));
        }
        for &index in &indexes {
            if index == self.primary_display || self.extra.iter().any(|p| p.display == index) {
                continue;
            }
            self.open_extra(event_loop, index);
        }
        if !self.placed_primary {
            if let Some(window) = self.window.clone() {
                if self.apply_place(event_loop, &window, self.primary_display) {
                    self.sized_to_remote = true;
                }
            }
            self.placed_primary = true;
        }
        if let Some(window) = &self.window {
            window.set_title(&self.screen_named(self.primary_display));
        }
        self.push_watch();
    }

    fn redraw_extra(&mut self, id: WindowId) {
        self.pump_frame();
        let Some(i) = self.extra.iter().position(|p| p.window.id() == id) else { return };
        let display = self.extra[i].display;
        let separate = self.is_separate();
        let picture = self.tile_picture(display);
        if !self.extra[i].sized {
            if let Some(frame) = picture.as_ref() {
                let window = self.extra[i].window.clone();
                Self::fit_remote(&window, frame.width, frame.height, 0);
                self.extra[i].sized = true;
            }
        }
        let toast = self.toast.clone();
        let background = self.toolbar.letterbox();
        let pane = &mut self.extra[i];
        let size = pane.window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if pane.surface.resize(width, height).is_err() {
            return;
        }
        let (win_w, win_h) = (size.width as usize, size.height as usize);
        let layout = {
            let Ok(mut buffer) = pane.surface.buffer_mut() else { return };
            let layout =
                Self::paint_picture(&mut buffer, win_w, win_h, picture.as_ref(), Zoom::Fit, (0, 0), 0, background);
            if picture.is_none() {
                toolbar::draw_text(&mut buffer, win_w, win_h, 24.0, 48.0, "Waiting for this screen", 0xFF_9a_a3_b5);
            }
            if separate {
                toolbar::draw_compact(&mut buffer, win_w, win_h, &format!("Screen {}", display + 1));
            }
            if let Some((text, at)) = &toast {
                if at.elapsed() < Duration::from_secs(4) {
                    toolbar::draw_toast(&mut buffer, win_w, win_h, text);
                }
            }
            let _ = buffer.present();
            layout
        };
        pane.layout = layout;
    }

    fn extra_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(display) = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.display) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.remember_places(true);
                self.extra.retain(|p| p.window.id() != id);
                self.push_watch();
                self.show_toast(format!("Screen {} closed", display + 1));
                if self.window.is_none() && self.extra.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.redraw_extra(id),
            WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                self.remember_places(false);
                if let Some(pane) = self.extra.iter().find(|p| p.window.id() == id) {
                    pane.window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => self.release_all(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let focused = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.window.clone());
                self.on_key(event_loop, event, focused);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(pane) = self.extra.iter_mut().find(|p| p.window.id() == id) {
                    pane.cursor = (position.x, position.y);
                }
                let size = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.window.inner_size());
                let Some(size) = size else { return };
                let (w, h) = (size.width as usize, size.height as usize);
                if toolbar::compact_covers(position.x, position.y, w, h) {
                    if let Some(pane) = self.extra.iter().find(|p| p.window.id() == id) {
                        pane.window.request_redraw();
                    }
                    return;
                }
                if self.toolbar.block_input {
                    return;
                }
                if let Some((x, y)) = self.pointer_for(display, position, size.width as f64, size.height as f64) {
                    self.send_pointer(display, x, y);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let size = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.window.inner_size());
                let cursor = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.cursor).unwrap_or((0.0, 0.0));
                let Some(size) = size else { return };
                let (w, h) = (size.width as usize, size.height as usize);
                if button == winit::event::MouseButton::Left && toolbar::compact_covers(cursor.0, cursor.1, w, h) {
                    if state == ElementState::Pressed {
                        if let Some(action) = toolbar::compact_click(cursor.0, cursor.1, w, h) {
                            if action == Action::CloseScreen {
                                self.remember_places(true);
                                self.extra.retain(|p| p.window.id() != id);
                                self.push_watch();
                                self.show_toast(format!("Screen {} closed", display + 1));
                            } else if action == Action::Fullscreen {
                                if let Some(pane) = self.extra.iter().find(|p| p.window.id() == id) {
                                    Self::toggle_fullscreen(&pane.window);
                                }
                            }
                        }
                    }
                    return;
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
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.toolbar.block_input {
                    self.on_wheel(delta);
                }
            }
            _ => {}
        }
    }

    fn focus_display(&self, index: u8) {
        if index == self.primary_display {
            if let Some(window) = &self.window {
                window.focus_window();
            }
            return;
        }
        if let Some(pane) = self.extra.iter().find(|p| p.display == index) {
            pane.window.focus_window();
        }
    }
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
        if self.follow_system_theme {
            self.toolbar.set_light(matches!(window.theme(), Some(winit::window::Theme::Light)));
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(window) = self.window.clone() else { return };
        match event {
            UserEvent::Frame => {
                if self.live.lock().unwrap().restart {
                    self.restarting = true;
                    event_loop.exit();
                    return;
                }
                self.sync_separate(event_loop);
                window.request_redraw();
                for pane in &self.extra {
                    pane.window.request_redraw();
                }
            }
            UserEvent::Title(text) => {
                if self.closed {
                    return;
                }
                if self.is_separate() {
                    window.set_title(&self.screen_named(self.primary_display));
                    for pane in &self.extra {
                        pane.window.set_title(&self.screen_named(pane.display));
                    }
                } else {
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.extra.iter().any(|p| p.window.id() == id) {
            self.extra_event(event_loop, id, event);
            return;
        }
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
            self.remember_places(false);
        }
        match event {
            WindowEvent::CloseRequested => {
                self.remember_places(true);
                if self.is_separate() {
                    if self.close_primary_screen() {
                        event_loop.exit();
                    }
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            // Don't leave keys "held" on the host when we lose focus.
            WindowEvent::Focused(false) => self.release_all(),
            WindowEvent::ThemeChanged(theme) => {
                if self.follow_system_theme {
                    self.toolbar.set_light(matches!(theme, winit::window::Theme::Light));
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                self.on_key(event_loop, event, self.window.clone())
            }
            WindowEvent::CursorMoved { position, .. } => self.on_cursor(position),
            WindowEvent::MouseInput { state, button, .. } => {
                // Clicks on the toolbar (or its open menu) are handled locally.
                if button == winit::event::MouseButton::Left {
                    let win_w = self.window.as_ref().map(|w| w.inner_size().width as usize).unwrap_or(0);
                    let win_h = self.window.as_ref().map(|w| w.inner_size().height as usize).unwrap_or(0);
                    if self.panel.covers(self.cursor_px.0, self.cursor_px.1, win_w, win_h) {
                        if state == ElementState::Pressed {
                            let snap = self.live.lock().unwrap().snapshot();
                            if let Some(cmd) = self.panel.click(self.cursor_px.0, self.cursor_px.1, win_w, win_h, &snap)
                            {
                                self.apply_panel(cmd);
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        return;
                    }
                    if self.is_separate() && toolbar::compact_covers(self.cursor_px.0, self.cursor_px.1, win_w, win_h) {
                        if state == ElementState::Pressed {
                            if let Some(action) =
                                toolbar::compact_click(self.cursor_px.0, self.cursor_px.1, win_w, win_h)
                            {
                                if self.perform(action) {
                                    event_loop.exit();
                                } else {
                                    self.sync_separate(event_loop);
                                }
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
                                } else {
                                    self.sync_separate(event_loop);
                                }
                            }
                            if let Some(window) = &self.window {
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
                if self.remote_is_android() {
                    if button == MouseButton::Left {
                        self.on_android_button(state == ElementState::Pressed);
                    }
                } else {
                    self.send(InputEvent::MouseButton { button, down: state == ElementState::Pressed });
                }
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
