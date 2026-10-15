//! The viewer window (winit + softbuffer) and its network/decoder plumbing.
//!
//! ```text
//!  QUIC video stream ─► receiver task ─► decoder thread ─► latest frame ─► window (redraw)
//!                                             │ FrameAck
//!  window input events ───────────────────────┴──────────► sender task ─► QUIC control stream
//! ```
//!
//! Local shortcut (not sent to the host): Ctrl+Alt+Enter toggles full screen.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use desklink_proto::format_id;
use desklink_proto::framing::{read_msg, MAX_CONTROL_FRAME};
use desklink_proto::peer::{read_video_frame, ControlMsg, HostMsg, InputEvent, MouseButton, VideoHeader};
use quinn::{Connection, SendStream};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CustomCursor, Fullscreen, Window, WindowId};

use super::decoder::VideoDecoder;
use super::render::{self, Layout, RgbFrame};
use super::{keymap, Link};

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

pub fn run(runtime: Runtime, link: Link) -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let slot = Arc::new(FrameSlot::default());
    let stats = Arc::new(Stats::default());
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlMsg>();
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<(VideoHeader, Vec<u8>)>();

    let Link { connection, endpoint, control_send, mut control_recv, host_name, host_id, path } = link;
    let title = format!("{host_name} ({}) — DeskLink, {path}", format_id(&host_id));

    runtime.spawn(send_control(control_send, control_rx));
    runtime.spawn(async move {
        // Host → controller messages (pongs, notices).
        while let Ok(msg) = read_msg::<_, HostMsg>(&mut control_recv, MAX_CONTROL_FRAME).await {
            if let HostMsg::Notice(text) = msg {
                info!("Host: {text}");
            }
        }
    });
    runtime.spawn(receive_video(connection.clone(), frame_tx, stats.clone(), proxy.clone()));
    runtime.spawn(report_stats(connection.clone(), stats.clone(), proxy.clone(), title.clone()));
    {
        let (slot, control_tx, proxy, stats) = (slot.clone(), control_tx.clone(), proxy.clone(), stats.clone());
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decode_loop(frame_rx, slot, control_tx, proxy, stats))?;
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
        keys_down: HashSet::new(),
        modifiers: ModifiersState::empty(),
        last_position: None,
        wheel: (0.0, 0.0),
        closed: false,
    };
    event_loop.run_app(&mut app)?;

    // Window closed: say goodbye so the host stops streaming immediately.
    connection.close(0u32.into(), b"viewer closed");
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    });
    Ok(())
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

async fn receive_video(
    connection: Connection,
    frames: std::sync::mpsc::Sender<(VideoHeader, Vec<u8>)>,
    stats: Arc<Stats>,
    proxy: EventLoopProxy<UserEvent>,
) {
    let result: Result<()> = async {
        let mut video = connection.accept_uni().await?;
        loop {
            let mut data = Vec::new();
            let header = read_video_frame(&mut video, &mut data).await?;
            stats.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            stats.encode_us.store(header.encode_us as u64, Ordering::Relaxed);
            if frames.send((header, data)).is_err() {
                return Ok(());
            }
        }
    }
    .await;
    let reason = match connection.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(close)) => {
            format!("the host ended the session ({})", String::from_utf8_lossy(&close.reason))
        }
        Some(quinn::ConnectionError::TimedOut) => "the connection timed out".into(),
        Some(other) => other.to_string(),
        None => match result {
            Err(e) => format!("{e:#}"),
            Ok(()) => "closed".into(),
        },
    };
    let _ = proxy.send_event(UserEvent::Closed(reason));
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
    keys_down: HashSet<u16>,
    modifiers: ModifiersState,
    last_position: Option<(u16, u16)>,
    /// Fractional wheel movement not sent yet (touchpads).
    wheel: (f64, f64),
    closed: bool,
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
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        match &self.current {
            Some(frame) => self.layout = render::draw(frame, &mut buffer, size.width as usize, size.height as usize),
            None => buffer.fill(0x00_1e_1e_22),
        }
        let _ = buffer.present();
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let down = event.state == ElementState::Pressed;

        if down && code == KeyCode::Enter && self.modifiers.control_key() && self.modifiers.alt_key() {
            if let Some(window) = &self.window {
                let fullscreen = window.fullscreen().is_none();
                window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
            }
            let _ = event_loop;
            return;
        }
        let Some(scan) = keymap::scancode(code) else { return };
        if down {
            self.keys_down.insert(scan);
        } else {
            self.keys_down.remove(&scan);
        }
        self.send(InputEvent::Key { scan, down });
    }

    fn on_cursor(&mut self, position: PhysicalPosition<f64>) {
        let layout = self.layout;
        if layout.width == 0 || layout.height == 0 {
            return;
        }
        let fx = ((position.x - layout.x as f64) / layout.width as f64).clamp(0.0, 1.0);
        let fy = ((position.y - layout.y as f64) / layout.height as f64).clamp(0.0, 1.0);
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
            UserEvent::Frame => window.request_redraw(),
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
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            _ => {}
        }
    }
}
