//! The viewer window (winit + softbuffer) and its network/decoder plumbing.
//!
//! ```text
//!  QUIC video stream ─► receiver task ─► decoder thread ─► latest frame ─► window (redraw)
//!                                             │ FrameAck
//!  window input events ───────────────────────┴──────────► sender task ─► QUIC control stream
//! ```
//!
//! Local shortcut (not sent to the host): Ctrl+Alt+Enter toggles full screen.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pjdesk_proto::format_id;
use pjdesk_proto::peer::{ControlMsg, InputEvent, MouseButton, VideoHeader, KEY_NUMPAD_DIGIT};
use quinn::{Connection, Endpoint, SendStream};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CustomCursor, Fullscreen, UserAttentionType, Window, WindowId};

use super::decoder::VideoDecoder;
use super::render::{self, Layout, RgbFrame};
use super::toolbar::{self, Action, Combo};
use super::{keymap, Link};
use pjdesk_proto::peer::SpecialCommand;

/// Events delivered to the window from other threads.
#[derive(Debug)]
enum UserEvent {
    /// A new decoded frame is ready for this session.
    Frame(u64),
    /// New title text (statistics) for this session.
    Title { session: u64, text: String },
    /// The connection ended for this session.
    Closed { session: u64, reason: String },
    /// Files dragged onto the viewer from Explorer.
    Files(Vec<std::path::PathBuf>),
    /// Another computer was accepted, or a connect finished.
    Work,
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

/// Enough to open the same computer again without asking for the password.
///
/// The password stays in this process. It is not written to the log.
#[derive(Clone)]
pub(crate) struct Seed {
    pub server: String,
    pub target: String,
    pub password: String,
    pub direct: Option<String>,
    pub force_relay: bool,
    pub display: u8,
    pub view: u8,
    pub remember: bool,
    pub from_store: bool,
    pub files: bool,
    pub label: String,
    /// Addresses that reached this computer without the relay.
    pub direct_targets: Vec<std::net::SocketAddr>,
    pub host_fingerprint: Option<pjdesk_proto::tls::Fingerprint>,
}

#[derive(Clone)]
enum SessionCover {
    Reconnecting,
    Lost,
    Ended(String),
}

/// When the next automatic dial for this tab is due, and how long that wait was.
struct RetryWait {
    next_at: Instant,
    delay: Duration,
}

pub enum ViewerStop {
    /// The person closed the window.
    UserClosed,
    /// The host is restarting. Wait, then connect again.
    HostRestart,
    /// A relayed session is moving to the primary. Connect again with the same password.
    /// `primary` is empty when the account's current primary should be used.
    RelayResume { primary: String },
}

struct Armed {
    title: String,
    slot: Arc<FrameSlot>,
    control: mpsc::UnboundedSender<ControlMsg>,
    live: Arc<Mutex<super::controls::Live>>,
    cmds: mpsc::UnboundedSender<super::controls::Cmd>,
    saw_picture: Arc<AtomicBool>,
    host_name: String,
    peer_id: String,
    connection: Connection,
    endpoint: Endpoint,
    retire: Arc<AtomicBool>,
}

enum Queued {
    Admit(crate::session_window::Admit),
    Linked { sid: u64, link: Link, seed: Seed },
    Failed { sid: u64, error: String, id: String, files: bool, from_store: bool },
    Relinked { sid: u64, link: Link, gen: u64 },
    RelinkFailed { sid: u64, error: String, gen: u64 },
}

fn arm_link(
    runtime: &tokio::runtime::Handle,
    link: Link,
    proxy: &EventLoopProxy<UserEvent>,
    sid: u64,
    view: u8,
    display: u8,
) -> Result<Armed> {
    if let Some(reason) = link.connection.close_reason() {
        let text = format!("The host closed the session before the window opened: {reason}");
        crate::note_failure(&text);
        anyhow::bail!(text);
    }
    let slot = Arc::new(FrameSlot::default());
    let stats = Arc::new(Stats::default());
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlMsg>();
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<(VideoHeader, Vec<u8>)>();
    let Link { connection, endpoint, control_send, control_recv, host_name, host_id, path, peer_proto, .. } = link;
    let relayed = matches!(path, crate::p2p::LinkPath::Relay(_));
    let title = format!("{host_name} ({}) — PJDesk, {path}", format_id(&host_id));
    let retire = Arc::new(AtomicBool::new(false));
    runtime.spawn(send_control(control_send, control_rx));
    let wake_proxy = proxy.clone();
    let (live, cmds, recorder) = super::controls::start(
        runtime.clone(),
        connection.clone(),
        control_recv,
        control_tx.clone(),
        frame_tx,
        peer_proto,
        host_id.clone(),
        host_name.clone(),
        relayed,
        Some(view),
        Some(display),
        move || {
            let _ = wake_proxy.send_event(UserEvent::Frame(sid));
        },
    );
    runtime.spawn(report_stats(connection.clone(), stats.clone(), proxy.clone(), title.clone(), sid, retire.clone()));
    {
        let (slot, control_tx, proxy, stats, recorder, retire) =
            (slot.clone(), control_tx.clone(), proxy.clone(), stats.clone(), recorder.clone(), retire.clone());
        std::thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decode_loop(frame_rx, slot, control_tx, proxy, stats, recorder, sid, retire))?;
    }
    let saw_picture = Arc::new(AtomicBool::new(false));
    let saw_watch = saw_picture.clone();
    let live_watch = live.clone();
    let host_for_watch = host_name.clone();
    let proxy_watch = proxy.clone();
    let retire_watch = retire.clone();
    runtime.spawn(async move {
        watch_picture(sid, saw_watch, live_watch, host_for_watch, proxy_watch, retire_watch).await;
    });
    Ok(Armed {
        title,
        slot,
        control: control_tx,
        live,
        cmds,
        saw_picture,
        host_name,
        peer_id: host_id,
        connection,
        endpoint,
        retire,
    })
}

async fn watch_picture(
    session: u64,
    saw: Arc<AtomicBool>,
    live: Arc<Mutex<super::controls::Live>>,
    host_name: String,
    proxy: EventLoopProxy<UserEvent>,
    retire: Arc<AtomicBool>,
) {
    let started = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if retire.load(Ordering::SeqCst) || saw.load(Ordering::Relaxed) {
            return;
        }
        let guard = live.lock().unwrap();
        if guard.resume_relay {
            drop(guard);
            if !retire.load(Ordering::SeqCst) {
                let _ = proxy.send_event(UserEvent::Closed {
                    session,
                    reason: pjdesk_proto::failover::RELAY_RESUME.into(),
                });
            }
            return;
        }
        if guard.restart {
            return;
        }
        if let Some(text) = guard.closed.clone() {
            drop(guard);
            if !retire.load(Ordering::SeqCst) {
                let _ = proxy.send_event(UserEvent::Closed { session, reason: text });
            }
            return;
        }
        let notices = guard.notices.clone();
        drop(guard);
        let limit = super::controls::picture_limit_secs(&notices);
        if started.elapsed() < Duration::from_secs(limit) {
            continue;
        }
        if saw.load(Ordering::Relaxed) || retire.load(Ordering::SeqCst) {
            return;
        }
        let text = super::controls::picture_gave_up(&host_name, &notices);
        let mut guard = live.lock().unwrap();
        if guard.closed.is_some() || guard.restart || saw.load(Ordering::Relaxed) || retire.load(Ordering::SeqCst) {
            return;
        }
        crate::note_failure(&text);
        guard.closed = Some(text.clone());
        drop(guard);
        let _ = proxy.send_event(UserEvent::Closed { session, reason: text });
        return;
    }
}

fn serve_admits(
    stop: Arc<AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
    queue: Arc<Mutex<Vec<Queued>>>,
) {
    let listener = match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(e) => {
            warn!("session tabs: could not listen for another computer: {e}");
            return;
        }
    };
    let Ok(port) = listener.local_addr().map(|addr| addr.port()) else { return };
    if let Err(e) = crate::session_window::publish_listener(port) {
        warn!("session tabs: could not publish the listener: {e}");
        return;
    }
    info!("Session window is listening for more computers");
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut sock, addr)) => {
                if !addr.ip().is_loopback() {
                    continue;
                }
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = sock.set_write_timeout(Some(Duration::from_secs(2)));
                if let Some(admit) = crate::session_window::read_admit(&mut sock) {
                    let _ = sock.write_all(b"ok\n");
                    queue.lock().unwrap().push(Queued::Admit(admit));
                    let _ = proxy.send_event(UserEvent::Work);
                } else {
                    let _ = sock.write_all(b"no\n");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    crate::session_window::take_down_listener(port);
}

pub fn run(runtime: &Runtime, link: Link, mut seed: Seed) -> Result<ViewerStop> {
    crate::note_status("Opening the viewer window");
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().context("could not create the viewer window")?;
    let proxy = event_loop.create_proxy();
    let view = seed.view;
    let display = seed.display;
    if !link.direct_targets.is_empty() {
        seed.direct_targets = link.direct_targets.clone();
    }
    seed.host_fingerprint = link.host_fingerprint.or(seed.host_fingerprint);
    let armed = arm_link(runtime.handle(), link, &proxy, 1, view, display)?;
    let loaded = crate::config::Config::load().ok();
    let theme_name = loaded.as_ref().map(|c| c.settings.theme.clone()).unwrap_or_default();
    let mut toolbar = toolbar::Toolbar::default();
    let follow_system_theme = theme_name != "light" && theme_name != "dark";
    let light = theme_name == "light";
    if light {
        toolbar.set_light(true);
    }
    let host_id = armed.peer_id.clone();
    let places =
        loaded.as_ref().and_then(|c| c.settings.peers.get(&host_id).map(|n| n.windows.clone())).unwrap_or_default();
    let queue = Arc::new(Mutex::new(Vec::new()));
    let stop_listener = Arc::new(AtomicBool::new(false));
    let listener = {
        let stop = stop_listener.clone();
        let proxy = proxy.clone();
        let queue = queue.clone();
        std::thread::Builder::new().name("session-tabs".into()).spawn(move || serve_admits(stop, proxy, queue)).ok()
    };
    let mut app = App {
        title: armed.title,
        window: None,
        surface: None,
        slot: armed.slot,
        current: None,
        layout: Layout::default(),
        control: armed.control,
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
        pan_shown: (0.0, 0.0),
        pan_at: Instant::now(),
        dot_cursor: None,
        edge_cursor: false,
        last_title_click: None,
        live: armed.live,
        cmds: armed.cmds,
        panel: super::panel::Panel::default(),
        restarting: false,
        relay_resume: false,
        relay_primary: String::new(),
        peer_id: armed.peer_id,
        host_name: armed.host_name,
        extra: Vec::new(),
        primary_display: display,
        toast: None,
        sent_watch: Vec::new(),
        places,
        place_saved: Instant::now() - Duration::from_secs(5),
        placed_primary: false,
        follow_system_theme,
        noted_platform: String::new(),
        remote_size: None,
        fitted_extra: None,
        android_stroke: pjdesk_proto::gesture::StrokeTracker::default(),
        pointer_down: false,
        saw_picture: armed.saw_picture,
        failed_open: None,
        window_shown: Arc::new(AtomicBool::new(false)),
        proxy: proxy.clone(),
        sid: 1,
        seed,
        connection: armed.connection,
        endpoint: armed.endpoint,
        retire: armed.retire,
        others: Vec::new(),
        tab_order: vec![1],
        connecting: Vec::new(),
        cancelled: HashSet::new(),
        relinking: HashSet::new(),
        cover: HashMap::new(),
        retries: HashMap::new(),
        retry_drawn: None,
        link_gen: Arc::new(Mutex::new(HashMap::new())),
        resume_stop: Arc::new(Mutex::new(HashSet::new())),
        confirm_close: false,
        queue,
        rt: runtime.handle().clone(),
        next_sid: 2,
        light,
    };
    app.apply_tab_strip();
    if std::env::var("PJDESK_OPEN_PANEL").ok().as_deref() == Some("files") || app.seed.files {
        app.panel.open = true;
        app.panel.tab = 0;
    }
    let shown = app.window_shown.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        if !shown.load(Ordering::SeqCst) {
            crate::note_failure(&format!(
                "The viewer window did not open after the password was accepted. {}",
                crate::host_log_hint()
            ));
            std::process::exit(1);
        }
    });
    event_loop.run_app(&mut app)?;
    stop_listener.store(true, Ordering::SeqCst);
    if let Some(listener) = listener {
        let _ = listener.join();
    }
    if let Some(text) = app.failed_open.clone() {
        anyhow::bail!(text);
    }

    let stop = if app.relay_resume {
        ViewerStop::RelayResume { primary: app.relay_primary }
    } else if app.restarting {
        ViewerStop::HostRestart
    } else {
        ViewerStop::UserClosed
    };
    if !matches!(stop, ViewerStop::HostRestart | ViewerStop::RelayResume { .. }) {
        app.retire.store(true, Ordering::SeqCst);
        app.connection.close(0u32.into(), b"viewer closed");
        for other in &app.others {
            other.retire.store(true, Ordering::SeqCst);
            other.connection.close(0u32.into(), b"viewer closed");
        }
    }
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(1), app.endpoint.wait_idle()).await;
        for other in &app.others {
            let _ = tokio::time::timeout(Duration::from_secs(1), other.endpoint.wait_idle()).await;
        }
    });
    Ok(stop)
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

async fn report_stats(
    connection: Connection,
    stats: Arc<Stats>,
    proxy: EventLoopProxy<UserEvent>,
    title: String,
    session: u64,
    retire: Arc<AtomicBool>,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let (mut frames_before, mut bytes_before) = (0u64, 0u64);
    loop {
        tick.tick().await;
        if retire.load(Ordering::SeqCst) || connection.close_reason().is_some() {
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
        if proxy.send_event(UserEvent::Title { session, text }).is_err() {
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
    session: u64,
    retire: Arc<AtomicBool>,
) {
    let mut decoder = match VideoDecoder::new() {
        Ok(decoder) => decoder,
        Err(e) => {
            let _ = proxy.send_event(UserEvent::Closed {
                session,
                reason: format!("cannot start the H.264 decoder: {e:#}"),
            });
            return;
        }
    };
    let mut waiting_for_keyframe = false;
    let mut last_request = Instant::now() - Duration::from_secs(10);

    while let Ok((header, data)) = frames.recv() {
        if retire.load(Ordering::SeqCst) {
            return;
        }
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
                let _ = proxy.send_event(UserEvent::Frame(session));
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

struct Held {
    sid: u64,
    seed: Seed,
    title: String,
    slot: Arc<FrameSlot>,
    current: Option<RgbFrame>,
    layout: Layout,
    control: mpsc::UnboundedSender<ControlMsg>,
    sized_to_remote: bool,
    keys_down: HashMap<KeyCode, u16>,
    modifiers: ModifiersState,
    last_position: Option<(u16, u16)>,
    cursor_px: (f64, f64),
    wheel: (f64, f64),
    closed: bool,
    toolbar: super::toolbar::Toolbar,
    zoom: Zoom,
    pan: (i32, i32),
    live: Arc<Mutex<super::controls::Live>>,
    cmds: mpsc::UnboundedSender<super::controls::Cmd>,
    panel: super::panel::Panel,
    restarting: bool,
    relay_resume: bool,
    relay_primary: String,
    peer_id: String,
    host_name: String,
    extra: Vec<ExtraPane>,
    primary_display: u8,
    toast: Option<(String, Instant)>,
    sent_watch: Vec<u8>,
    places: Vec<crate::config::SavedWindow>,
    place_saved: Instant,
    placed_primary: bool,
    noted_platform: String,
    remote_size: Option<(usize, usize)>,
    fitted_extra: Option<u32>,
    android_stroke: pjdesk_proto::gesture::StrokeTracker,
    pointer_down: bool,
    saw_picture: Arc<AtomicBool>,
    connection: Connection,
    endpoint: Endpoint,
    retire: Arc<AtomicBool>,
}

fn retire_held(held: &Held) {
    held.retire.store(true, Ordering::SeqCst);
    held.connection.close(0u32.into(), b"tab closed");
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
    /// Pan drawn right now; eases toward `pan` so following the pointer is smooth.
    pan_shown: (f32, f32),
    pan_at: Instant,
    dot_cursor: Option<winit::window::CustomCursor>,
    /// The pointer shows a resize arrow over the window's edge.
    edge_cursor: bool,
    last_title_click: Option<Instant>,
    live: Arc<Mutex<super::controls::Live>>,
    cmds: mpsc::UnboundedSender<super::controls::Cmd>,
    panel: super::panel::Panel,
    restarting: bool,
    relay_resume: bool,
    relay_primary: String,
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
    /// Last extra height asked of the window (tabs plus the Android bar).
    fitted_extra: Option<u32>,
    /// Press on an Android picture, turned into a tap or a swipe on release.
    android_stroke: pjdesk_proto::gesture::StrokeTracker,
    pointer_down: bool,
    saw_picture: Arc<AtomicBool>,
    failed_open: Option<String>,
    window_shown: Arc<AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
    sid: u64,
    seed: Seed,
    connection: Connection,
    endpoint: Endpoint,
    retire: Arc<AtomicBool>,
    others: Vec<Held>,
    tab_order: Vec<u64>,
    connecting: Vec<(u64, String)>,
    cancelled: HashSet<u64>,
    relinking: HashSet<u64>,
    cover: HashMap<u64, SessionCover>,
    /// Automatic redials. Absent once the tab is connected or the person stopped it.
    retries: HashMap<u64, RetryWait>,
    /// Last countdown second drawn, so the wait loop does not redraw every turn.
    retry_drawn: Option<u64>,
    /// Bumped on every dial so a slower attempt cannot replace a newer one.
    link_gen: Arc<Mutex<HashMap<u64, u64>>>,
    resume_stop: Arc<Mutex<HashSet<u64>>>,
    confirm_close: bool,
    queue: Arc<Mutex<Vec<Queued>>>,
    rt: tokio::runtime::Handle,
    next_sid: u64,
    light: bool,
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
        self.drain_queue();
        self.sync_chat_panel();
        self.apply_tab_strip();
        self.pump_frame();
        let confirm = self.confirm_close;
        let open = self.open_count();
        let light = self.light;
        let separate = self.is_separate();
        let display = self.primary_display;
        let cropped = if separate { self.tile_picture(display) } else { None };
        let cover_paint = self.cover.get(&self.sid).cloned().map(|cover| {
            let detail = self.cover_detail(self.sid, &cover);
            (cover_kind(&cover), detail)
        });
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
        let snap = self.live.lock().unwrap().snapshot();
        self.toolbar.set_titlebar(window.fullscreen().is_none(), window.is_maximized(), &self.title);
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
            let extra = self.toolbar.tab_height() as u32
                + if self.toolbar.shows_android_nav() { toolbar::ANDROID_BAR_H as u32 } else { 0 };
            if self.remote_size != Some(size) || self.fitted_extra != Some(extra) {
                self.remote_size = Some(size);
                self.fitted_extra = Some(extra);
                self.sized_to_remote = true;
                Self::fit_remote(window, frame.width, frame.height, extra);
            }
        }

        let size = window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(width, height).is_err() {
            return;
        }
        let (win_w, win_h) = (size.width as usize, size.height as usize);
        let nav = self.toolbar.shows_android_nav();
        let bar_h = if nav { toolbar::ANDROID_BAR_H as usize } else { 0 };
        let paint_h = win_h.saturating_sub(bar_h).max(1);
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        let shown = if separate { cropped.as_ref() } else { self.current.as_ref() };
        let origin = self.toolbar.tab_height() as usize;
        let now = Instant::now();
        let dt = now.duration_since(self.pan_at).as_secs_f32().min(0.1);
        self.pan_at = now;
        self.pan_shown = render::ease_pan(self.pan_shown, self.pan, dt);
        let easing = self.pan_shown != (self.pan.0 as f32, self.pan.1 as f32);
        let drawn_pan = (self.pan_shown.0.round() as i32, self.pan_shown.1.round() as i32);
        let layout = Self::paint_picture(
            &mut buffer,
            win_w,
            paint_h,
            shown,
            self.zoom,
            drawn_pan,
            origin,
            self.toolbar.letterbox(),
            nav,
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
        } else if self.current.is_none() {
            let waiting = if let Some(reason) = &snap.closed {
                reason.clone()
            } else if let Some(note) = snap.notices.iter().rev().find(|note| super::controls::screen_share_waiting(note)) {
                note.clone()
            } else {
                format!("Waiting for the picture from {}", self.host_name)
            };
            toolbar::draw_text(&mut buffer, win_w, win_h, 24.0, origin as f32 + 48.0, &waiting, 0xFF_e7_ec_f5);
        }
        Self::paint_overlay(&self.toolbar, &self.toast, &mut buffer, win_w, win_h, display, separate, true);
        if nav {
            toolbar::fill_rect(
                &mut buffer,
                win_w,
                win_h,
                0.0,
                paint_h as f32,
                win_w as f32,
                bar_h as f32,
                self.toolbar.letterbox(),
            );
            toolbar::draw_android_bar(
                &mut buffer,
                win_w,
                win_h,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                snap.control_off,
            );
        }
        if snap.control_off {
            toolbar::draw_banner(
                &mut buffer,
                win_w,
                win_h,
                origin as f32,
                super::controls::CONTROL_OFF,
            );
        }
        self.panel.draw(&mut buffer, win_w, win_h, &snap);
        if confirm {
            toolbar::draw_confirm_close(&mut buffer, win_w, win_h, open.max(2), light);
        }
        if let Some((kind, detail)) = &cover_paint {
            toolbar::draw_session_cover(&mut buffer, win_w, win_h, *kind, detail, light);
        }
        if snap.restart {
            self.restarting = true;
        }
        let _ = buffer.present();
        if easing {
            window.request_redraw();
        }
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
                self.pan_shown = (0.0, 0.0);
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
                if tab == 1 && !self.live.lock().unwrap().chat_ready {
                    self.live.lock().unwrap().notices.push(pjdesk_proto::peer::CHAT_NEEDS_TIP.into());
                } else if self.panel.open && self.panel.tab == tab {
                    self.panel.open = false;
                } else {
                    self.panel.open = true;
                    self.panel.tab = tab;
                    self.panel.focus = if tab == 1 { 1 } else { 0 };
                }
            }
            Action::Disconnect => return self.end_sid(self.sid),
            Action::SelectSession(index) => self.focus_index(index),
            Action::CloseSession(index) => return self.end_index(index),
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
            Action::ToggleAndroidBar => self.toolbar.toggle_android_nav(),
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
        let bar = if self.toolbar.shows_android_nav() { toolbar::ANDROID_BAR_H as i32 } else { 0 };
        (size.width as i32, (size.height as i32 - origin - bar).max(1), origin)
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
        self.pan_shown = (self.pan.0 as f32, self.pan.1 as f32);
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
                if !g.chat_ready {
                    g.notices.push(pjdesk_proto::peer::CHAT_NEEDS_TIP.into());
                } else if let Some(text) = pjdesk_proto::peer::chat_line(&text) {
                    let name = super::controls::controller_chat_name();
                    let unix = pjdesk_proto::peer::chat_now();
                    g.chat.push(pjdesk_proto::peer::ChatEntry { mine: true, name: name.clone(), text: text.clone(), unix });
                    if g.chat.len() > 200 {
                        let drop_n = g.chat.len() - 200;
                        g.chat.drain(0..drop_n);
                    }
                    drop(g);
                    let _ = self.control.send(ControlMsg::ChatLine { text, name, unix });
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
        if self.confirm_close {
            if down && code == KeyCode::Escape {
                self.confirm_close = false;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }
        if self.cover.contains_key(&self.sid) {
            if down && code == KeyCode::Escape && matches!(self.cover.get(&self.sid), Some(SessionCover::Reconnecting)) {
                self.stop_retry(self.sid);
            }
            return;
        }

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
        if let Some(window) = self.window.clone() {
            let size = window.inner_size();
            let edge = if self.toolbar.has_titlebar() && !window.is_maximized() {
                resize_edge(position.x, position.y, size.width as f64, size.height as f64, window.scale_factor())
            } else {
                None
            };
            if let Some(dir) = edge {
                window.set_cursor(winit::window::CursorIcon::from(dir));
                self.edge_cursor = true;
                return;
            }
            if self.edge_cursor {
                self.edge_cursor = false;
                if let Some(dot) = &self.dot_cursor {
                    window.set_cursor(dot.clone());
                }
            }
        }
        if self.confirm_close {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
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
                self.pointer_for(self.primary_display, position, size.width as f64, size.height as f64, self.toolbar.tab_height())
            {
                self.send_pointer(self.primary_display, x, y);
            }
            return;
        }
        // When zoomed in past the window, the picture follows the pointer
        // proportionally (see `render::follow_pan`) and eases there on redraw.
        let mut target_layout = None;
        if let (Zoom::Scale(scale), Some(window), Some((fw, fh))) =
            (self.zoom, self.window.clone(), self.current.as_ref().map(|f| (f.width, f.height)))
        {
            let (ww, wh, origin) = self.content_extent();
            let iw = ((fw as f32 * scale).round() as i32).max(1);
            let ih = ((fh as f32 * scale).round() as i32).max(1);
            let next = (
                render::follow_pan(position.x, ww, iw),
                render::follow_pan(position.y - origin as f64, wh, ih),
            );
            if next != self.pan {
                self.pan = next;
                window.request_redraw();
            }
            let mut lay = render::placement(fw, fh, ww as usize, wh as usize, scale, self.pan);
            lay.y += origin;
            target_layout = Some(lay);
        }
        if self.toolbar.block_input {
            return;
        }
        // Map through where the picture is heading, not where the easing has
        // drawn it, so the remote pointer lands under the local one.
        let layout = target_layout.unwrap_or(self.layout);
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
            6 => SpecialCommand::Notifications,
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
        if self.modifiers.control_key() && code == KeyCode::KeyV {
            self.paste_android_clipboard();
            return;
        }
        if !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key() {
            if let Some(which) = android_nav_key(code).or_else(|| android_nav_named(&event.logical_key)) {
                let volume = which == 3 || which == 4;
                if !event.repeat || volume {
                    self.send_android(which);
                }
                return;
            }
        }
        if self.modifiers.control_key() || self.modifiers.alt_key() {
            return;
        }
        let chars = match code {
            KeyCode::Backspace => "\u{8}",
            KeyCode::Enter => "\n",
            KeyCode::Tab => "\t",
            KeyCode::ArrowLeft => "\u{E000}",
            KeyCode::ArrowRight => "\u{E001}",
            KeyCode::ArrowUp => "\u{E002}",
            KeyCode::ArrowDown => "\u{E003}",
            _ => "",
        };
        if !chars.is_empty() {
            let _ = self.control.send(ControlMsg::Text { chars: chars.into() });
            return;
        }
        if let Some(text) = &event.text {
            let text = text.to_string();
            if !text.is_empty() && !text.chars().all(|c| c.is_control()) {
                let _ = self.control.send(ControlMsg::Text { chars: text });
            }
        }
    }

    fn paste_android_clipboard(&mut self) {
        let Some(text) = crate::clip::get_text() else { return };
        if text.is_empty() || text.len() > pjdesk_proto::peer::MAX_CLIP_TEXT {
            return;
        }
        let _ = self.control.send(ControlMsg::Text { chars: text });
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
            self.saw_picture.store(true, Ordering::Relaxed);
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

    fn hook_drops(&self, window: &Window) {
        let proxy = self.proxy.clone();
        super::filedrop::attach(window, move |paths| {
            let _ = proxy.send_event(UserEvent::Files(paths));
        });
    }

    /// Sends every dropped file, and the files inside a dropped folder, to the host.
    fn accept_dropped(&mut self, paths: Vec<std::path::PathBuf>) {
        let mut n = 0usize;
        for path in &paths {
            for file in expand_drop(path) {
                let _ = self.cmds.send(super::controls::Cmd::SendFile(file));
                n += 1;
            }
        }
        self.panel.open = true;
        self.panel.tab = 0;
        if n == 0 {
            self.show_toast("That drop had no files to send");
        } else if n == 1 {
            self.show_toast("Sending 1 file to the other computer");
        } else {
            self.show_toast(format!("Sending {n} files to the other computer"));
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
        dock_bottom: bool,
    ) -> Layout {
        let content_h = win_h.saturating_sub(origin_y).max(1);
        match frame {
            Some(frame) => {
                let mut layout = match zoom {
                    Zoom::Fit => render::fit(frame.width, frame.height, win_w, content_h),
                    Zoom::Scale(scale) => render::placement(frame.width, frame.height, win_w, content_h, scale, pan),
                };
                if dock_bottom && layout.height > 0 && layout.height <= content_h {
                    layout.y = (content_h - layout.height) as i32;
                }
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

    fn pointer_for(&self, display: u8, position: PhysicalPosition<f64>, win_w: f64, win_h: f64, origin: f32) -> Option<(u16, u16)> {
        let screen = {
            let g = self.live.lock().unwrap();
            screens_of(&g.displays).into_iter().find(|s| s.index == display)?
        };
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

    /// The window's close button (system or PJDesk's own title bar).
    fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        self.remember_places(true);
        if crate::session_window::window_close_needs_confirm(self.open_count()) {
            self.confirm_close = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else if self.is_separate() {
            if self.close_primary_screen() {
                event_loop.exit();
            }
        } else {
            event_loop.exit();
        }
    }

    /// Presses on the window edge and on the title strip: resize, caption
    /// buttons, drag and double-click to maximize. True when handled.
    fn title_press(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(window) = self.window.clone() else { return false };
        if !self.toolbar.has_titlebar() {
            return false;
        }
        let size = window.inner_size();
        let (px, py) = self.cursor_px;
        if !window.is_maximized() {
            if let Some(dir) = resize_edge(px, py, size.width as f64, size.height as f64, window.scale_factor()) {
                let _ = window.drag_resize_window(dir);
                return true;
            }
        }
        match self.toolbar.title_hit(px, py, size.width as usize) {
            Some(toolbar::TitleHit::Caption(toolbar::Caption::Minimize)) => window.set_minimized(true),
            Some(toolbar::TitleHit::Caption(toolbar::Caption::Maximize)) => {
                window.set_maximized(!window.is_maximized());
            }
            Some(toolbar::TitleHit::Caption(toolbar::Caption::Close)) => self.request_close(event_loop),
            Some(toolbar::TitleHit::Drag) => {
                let double = self.last_title_click.is_some_and(|at| at.elapsed() < Duration::from_millis(450));
                if double {
                    self.last_title_click = None;
                    window.set_maximized(!window.is_maximized());
                } else {
                    self.last_title_click = Some(Instant::now());
                    let _ = window.drag_window();
                }
            }
            Some(toolbar::TitleHit::Tab) | None => return false,
        }
        window.request_redraw();
        true
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
            let dot = event_loop.create_custom_cursor(source);
            window.set_cursor(dot.clone());
            self.dot_cursor = Some(dot);
        }
        let Ok(context) = softbuffer::Context::new(window.clone()) else { return };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else { return };
        window.request_redraw();
        self.hook_drops(&window);
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
        let cover = self.cover.get(&self.sid).cloned();
        let cover_detail = cover.as_ref().map(|cover| self.cover_detail(self.sid, cover));
        let light = self.light;
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
                Self::paint_picture(&mut buffer, win_w, win_h, picture.as_ref(), Zoom::Fit, (0, 0), 0, background, false);
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
            if let Some(cover) = &cover {
                let detail = cover_detail.as_deref().unwrap_or("");
                toolbar::draw_session_cover(&mut buffer, win_w, win_h, cover_kind(cover), detail, light);
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
                if let Some((x, y)) = self.pointer_for(display, position, size.width as f64, size.height as f64, 0.0) {
                    self.send_pointer(display, x, y);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(cover) = self.cover.get(&self.sid).cloned() {
                    let size = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.window.inner_size());
                    let cursor = self.extra.iter().find(|p| p.window.id() == id).map(|p| p.cursor).unwrap_or((0.0, 0.0));
                    if let Some(size) = size {
                        let (w, h) = (size.width as usize, size.height as usize);
                        if state == ElementState::Pressed && button == winit::event::MouseButton::Left {
                            let kind = match cover {
                                SessionCover::Reconnecting => toolbar::SessionCover::Reconnecting,
                                SessionCover::Lost => toolbar::SessionCover::Lost,
                                SessionCover::Ended(_) => toolbar::SessionCover::Ended,
                            };
                            if let Some(hit) = toolbar::session_cover_hit(kind, cursor.0, cursor.1, w, h) {
                                let sid = self.sid;
                                match hit {
                                    toolbar::SessionCoverAction::Cancel => self.stop_retry(sid),
                                    toolbar::SessionCoverAction::Reconnect => self.reconnect_now(sid),
                                    toolbar::SessionCoverAction::Close => {
                                        if self.end_sid(sid) {
                                            event_loop.exit();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
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

    fn open_count(&self) -> usize {
        1 + self.others.len() + self.connecting.len()
    }

    fn live_ids(&self) -> Vec<u64> {
        let mut ids = vec![self.sid];
        ids.extend(self.others.iter().map(|held| held.sid));
        ids
    }

    fn sync_chat_panel(&mut self) {
        let open = self.panel.open && self.panel.tab == 1;
        let mut g = self.live.lock().unwrap();
        g.chat_open = open;
        if open {
            g.chat_unread = 0;
        }
        let ready = g.chat_ready;
        drop(g);
        self.toolbar.set_chat_ready(ready);
    }

    fn unread_of(&self, sid: u64) -> u32 {
        if sid == self.sid {
            return self.live.lock().unwrap().chat_unread;
        }
        self.others.iter().find(|held| held.sid == sid).map(|held| held.live.lock().unwrap().chat_unread).unwrap_or(0)
    }

    fn apply_tab_strip(&mut self) {
        let names: Vec<String> = self.tab_order.iter().copied().map(|sid| self.tab_label(sid)).collect();
        let unread: Vec<u32> = self.tab_order.iter().copied().map(|sid| self.unread_of(sid)).collect();
        let active = self.tab_order.iter().position(|sid| *sid == self.sid).unwrap_or(0);
        if names.is_empty() {
            self.toolbar.set_sessions(&[], 0);
            self.toolbar.set_unread(&[]);
        } else {
            self.toolbar.set_sessions(&names, active);
            self.toolbar.set_unread(&unread);
        }
    }

    fn tab_label(&self, sid: u64) -> String {
        if sid == self.sid {
            return session_name(&self.host_name, &self.peer_id, &self.seed.label);
        }
        if let Some(held) = self.others.iter().find(|held| held.sid == sid) {
            return session_name(&held.host_name, &held.peer_id, &held.seed.label);
        }
        if let Some((_, name)) = self.connecting.iter().find(|(id, _)| *id == sid) {
            return name.clone();
        }
        "Session".into()
    }

    fn reveal(&self) {
        if let Some(window) = &self.window {
            window.set_title(&self.title);
            window.request_redraw();
            window.focus_window();
        }
    }

    fn take_active(&mut self, next: Held) -> Held {
        let held = self.take_active_inner(next);
        self.pan_shown = (self.pan.0 as f32, self.pan.1 as f32);
        held
    }

    fn take_active_inner(&mut self, next: Held) -> Held {
        Held {
            sid: std::mem::replace(&mut self.sid, next.sid),
            seed: std::mem::replace(&mut self.seed, next.seed),
            title: std::mem::replace(&mut self.title, next.title),
            slot: std::mem::replace(&mut self.slot, next.slot),
            current: std::mem::replace(&mut self.current, next.current),
            layout: std::mem::replace(&mut self.layout, next.layout),
            control: std::mem::replace(&mut self.control, next.control),
            sized_to_remote: std::mem::replace(&mut self.sized_to_remote, next.sized_to_remote),
            keys_down: std::mem::replace(&mut self.keys_down, next.keys_down),
            modifiers: std::mem::replace(&mut self.modifiers, next.modifiers),
            last_position: std::mem::replace(&mut self.last_position, next.last_position),
            cursor_px: std::mem::replace(&mut self.cursor_px, next.cursor_px),
            wheel: std::mem::replace(&mut self.wheel, next.wheel),
            closed: std::mem::replace(&mut self.closed, next.closed),
            toolbar: std::mem::replace(&mut self.toolbar, next.toolbar),
            zoom: std::mem::replace(&mut self.zoom, next.zoom),
            pan: std::mem::replace(&mut self.pan, next.pan),
            live: std::mem::replace(&mut self.live, next.live),
            cmds: std::mem::replace(&mut self.cmds, next.cmds),
            panel: std::mem::replace(&mut self.panel, next.panel),
            restarting: std::mem::replace(&mut self.restarting, next.restarting),
            relay_resume: std::mem::replace(&mut self.relay_resume, next.relay_resume),
            relay_primary: std::mem::replace(&mut self.relay_primary, next.relay_primary),
            peer_id: std::mem::replace(&mut self.peer_id, next.peer_id),
            host_name: std::mem::replace(&mut self.host_name, next.host_name),
            extra: std::mem::replace(&mut self.extra, next.extra),
            primary_display: std::mem::replace(&mut self.primary_display, next.primary_display),
            toast: std::mem::replace(&mut self.toast, next.toast),
            sent_watch: std::mem::replace(&mut self.sent_watch, next.sent_watch),
            places: std::mem::replace(&mut self.places, next.places),
            place_saved: std::mem::replace(&mut self.place_saved, next.place_saved),
            placed_primary: std::mem::replace(&mut self.placed_primary, next.placed_primary),
            noted_platform: std::mem::replace(&mut self.noted_platform, next.noted_platform),
            remote_size: std::mem::replace(&mut self.remote_size, next.remote_size),
            fitted_extra: std::mem::replace(&mut self.fitted_extra, next.fitted_extra),
            android_stroke: std::mem::replace(&mut self.android_stroke, next.android_stroke),
            pointer_down: std::mem::replace(&mut self.pointer_down, next.pointer_down),
            saw_picture: std::mem::replace(&mut self.saw_picture, next.saw_picture),
            connection: std::mem::replace(&mut self.connection, next.connection),
            endpoint: std::mem::replace(&mut self.endpoint, next.endpoint),
            retire: std::mem::replace(&mut self.retire, next.retire),
        }
    }

    fn install(&mut self, next: Held) {
        hide_panes(&mut self.extra);
        let previous = self.take_active(next);
        previous.live.lock().unwrap().chat_open = false;
        self.others.push(previous);
        show_panes(&self.extra);
        self.apply_tab_strip();
        self.reveal();
    }

    fn focus_index(&mut self, index: usize) {
        let Some(&sid) = self.tab_order.get(index) else { return };
        if sid == self.sid {
            if let Some(window) = &self.window {
                window.focus_window();
            }
            return;
        }
        if self.connecting.iter().any(|(id, _)| *id == sid) {
            self.show_toast("Connecting…");
            return;
        }
        let Some(pos) = self.others.iter().position(|held| held.sid == sid) else { return };
        let next = self.others.remove(pos);
        hide_panes(&mut self.extra);
        let previous = self.take_active(next);
        previous.live.lock().unwrap().chat_open = false;
        self.others.push(previous);
        show_panes(&self.extra);
        self.apply_tab_strip();
        self.reveal();
    }

    fn end_index(&mut self, index: usize) -> bool {
        let Some(&sid) = self.tab_order.get(index) else { return false };
        self.end_sid(sid)
    }

    /// Ends one computer. True when that was the last live session and the window should close.
    fn end_sid(&mut self, sid: u64) -> bool {
        let order = self.tab_order.clone();
        let rest: Vec<u64> = self.live_ids().into_iter().filter(|id| *id != sid).collect();
        self.tab_order.retain(|id| *id != sid);
        if self.connecting.iter().any(|(id, _)| *id == sid) {
            self.connecting.retain(|(id, _)| *id != sid);
            self.cancelled.insert(sid);
            self.apply_tab_strip();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }
        if sid != self.sid {
            if let Some(held) = self.others.iter().find(|held| held.sid == sid) {
                retire_held(held);
            }
            self.others.retain(|held| held.sid != sid);
            self.relinking.remove(&sid);
            self.cover.remove(&sid);
            self.retries.remove(&sid);
            self.resume_stop.lock().unwrap().insert(sid);
            self.apply_tab_strip();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }
        let Some(next_sid) = crate::session_window::focus_after_close(&order, sid, &rest) else {
            return true;
        };
        let Some(pos) = self.others.iter().position(|held| held.sid == next_sid) else { return true };
        let next = self.others.remove(pos);
        hide_panes(&mut self.extra);
        let previous = self.take_active(next);
        retire_held(&previous);
        drop(previous);
        show_panes(&self.extra);
        self.relinking.remove(&sid);
        self.cover.remove(&sid);
        self.retries.remove(&sid);
        self.resume_stop.lock().unwrap().insert(sid);
        self.apply_tab_strip();
        self.reveal();
        false
    }

    /// True when this frame already switched the window (exit or an in-window reconnect).
    fn note_transport(&mut self, event_loop: &ActiveEventLoop, session: u64) -> bool {
        let Some((restart, resume, primary, ended)) = self.transport_flags(session) else { return false };
        if restart {
            return self.leave_or_relink(event_loop, session, String::new(), true);
        }
        if resume {
            return self.leave_or_relink(event_loop, session, primary, false);
        }
        if let Some(text) = ended {
            self.show_cover(session, SessionCover::Ended(text));
            return true;
        }
        false
    }

    fn relay_or_restart(&mut self, event_loop: &ActiveEventLoop, session: u64, reason: &str) -> bool {
        let relay = pjdesk_proto::failover::is_relay_resume(reason)
            || self.transport_flags(session).is_some_and(|(_, resume, _, _)| resume);
        if !relay {
            return false;
        }
        let mut primary = self.transport_flags(session).map(|(_, _, primary, _)| primary).unwrap_or_default();
        if primary.is_empty() {
            primary = pjdesk_proto::failover::primary_from_resume(reason);
        }
        self.leave_or_relink(event_loop, session, primary, false)
    }

    fn transport_flags(&self, session: u64) -> Option<(bool, bool, String, Option<String>)> {
        let pack = |live: &super::controls::Live| {
            let ended = if live.host_ended { live.closed.clone() } else { None };
            (live.restart, live.resume_relay, live.move_primary.clone(), ended)
        };
        if session == self.sid {
            let live = self.live.lock().unwrap();
            return Some(pack(&live));
        }
        let held = self.others.iter().find(|held| held.sid == session)?;
        let live = held.live.lock().unwrap();
        Some(pack(&live))
    }

    fn leave_or_relink(&mut self, event_loop: &ActiveEventLoop, session: u64, primary: String, restart: bool) -> bool {
        if restart && self.live_ids().len() <= 1 {
            self.restarting = true;
            event_loop.exit();
            return true;
        }
        self.begin_relink(session, primary, restart);
        true
    }

    fn show_cover(&mut self, sid: u64, cover: SessionCover) {
        self.cover.insert(sid, cover);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn stop_retry(&mut self, sid: u64) {
        self.resume_stop.lock().unwrap().insert(sid);
        self.relinking.remove(&sid);
        self.retries.remove(&sid);
        self.show_cover(sid, SessionCover::Ended("Stopped reconnecting.".into()));
    }

    fn reconnect_now(&mut self, sid: u64) {
        self.resume_stop.lock().unwrap().insert(sid);
        self.relinking.remove(&sid);
        self.retries.insert(sid, RetryWait { next_at: Instant::now(), delay: Duration::from_secs(1) });
        self.begin_relink(sid, String::new(), false);
    }

    fn begin_relink(&mut self, sid: u64, primary: String, restart: bool) {
        if !self.relinking.insert(sid) {
            return;
        }
        let gen = {
            let mut gens = self.link_gen.lock().unwrap();
            let gen = gens.get(&sid).copied().unwrap_or(0).wrapping_add(1);
            gens.insert(sid, gen);
            gen
        };
        self.resume_stop.lock().unwrap().remove(&sid);
        self.show_cover(sid, SessionCover::Reconnecting);
        let seed = if sid == self.sid {
            self.retire.store(true, Ordering::SeqCst);
            self.connection.close(0u32.into(), b"reconnecting");
            self.seed.clone()
        } else if let Some(held) = self.others.iter_mut().find(|held| held.sid == sid) {
            held.retire.store(true, Ordering::SeqCst);
            held.connection.close(0u32.into(), b"reconnecting");
            held.seed.clone()
        } else {
            self.relinking.remove(&sid);
            self.cover.remove(&sid);
            self.retries.remove(&sid);
            return;
        };
        let rt = self.rt.clone();
        let queue = self.queue.clone();
        let proxy = self.proxy.clone();
        let stop = self.resume_stop.clone();
        let gens = self.link_gen.clone();
        let targets = seed.direct_targets.clone();
        let fingerprint = seed.host_fingerprint;
        rt.spawn(async move {
            if restart {
                tokio::time::sleep(Duration::from_secs(5)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            if stop.lock().unwrap().contains(&sid) || gens.lock().unwrap().get(&sid).copied() != Some(gen) {
                return;
            }
            let admit = admit_from_seed(&seed);
            let moved = if primary.trim().is_empty() { None } else { Some(primary) };
            let msg = match super::resume_session(&admit, moved.as_deref(), &targets, fingerprint).await {
                Ok(link) => {
                    if stop.lock().unwrap().contains(&sid) || gens.lock().unwrap().get(&sid).copied() != Some(gen) {
                        link.connection.close(0u32.into(), b"cancelled");
                        return;
                    }
                    Queued::Relinked { sid, link, gen }
                }
                Err(e) => {
                    if stop.lock().unwrap().contains(&sid) || gens.lock().unwrap().get(&sid).copied() != Some(gen) {
                        return;
                    }
                    Queued::RelinkFailed { sid, error: format!("{e:#}"), gen }
                }
            };
            queue.lock().unwrap().push(msg);
            let _ = proxy.send_event(UserEvent::Work);
        });
    }

    fn drain_queue(&mut self) {
        let batch = std::mem::take(&mut *self.queue.lock().unwrap());
        for msg in batch {
            self.handle_queued(msg);
        }
    }

    fn handle_queued(&mut self, msg: Queued) {
        match msg {
            Queued::Admit(admit) => self.start_admit(admit),
            Queued::Linked { sid, link, seed } => self.finish_link(sid, link, seed),
            Queued::Failed { sid, error, id, files, from_store } => self.fail_link(sid, error, id, files, from_store),
            Queued::Relinked { sid, link, gen } => self.finish_relink(sid, link, gen),
            Queued::RelinkFailed { sid, error, gen } => self.fail_relink(sid, error, gen),
        }
    }

    fn start_admit(&mut self, admit: crate::session_window::Admit) {
        let sid = self.next_sid;
        self.next_sid += 1;
        let label = if !admit.label.trim().is_empty() {
            admit.label.clone()
        } else if !admit.direct.trim().is_empty() {
            admit.direct.clone()
        } else {
            format_id(&admit.id)
        };
        let shown = if admit.direct.trim().is_empty() { format_id(&admit.id) } else { admit.direct.clone() };
        info!("Adding a session tab for {shown}");
        self.connecting.push((sid, label));
        self.tab_order.push(sid);
        self.apply_tab_strip();
        let seed = seed_from_admit(&admit);
        let rt = self.rt.clone();
        let queue = self.queue.clone();
        let proxy = self.proxy.clone();
        rt.spawn(async move {
            let id = admit.id.clone();
            let files = admit.files;
            let from_store = admit.from_store;
            let msg = match super::open_admitted(&admit, None, false).await {
                Ok(link) => Queued::Linked { sid, link, seed },
                Err(e) => Queued::Failed { sid, error: format!("{e:#}"), id, files, from_store },
            };
            queue.lock().unwrap().push(msg);
            let _ = proxy.send_event(UserEvent::Work);
        });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn finish_link(&mut self, sid: u64, link: Link, seed: Seed) {
        self.connecting.retain(|(id, _)| *id != sid);
        if self.cancelled.contains(&sid) || !self.tab_order.contains(&sid) {
            link.connection.close(0u32.into(), b"tab closed");
            return;
        }
        if seed.remember && !seed.password.is_empty() && pjdesk_proto::is_valid_id(&seed.target) {
            if let Ok(mut cfg) = crate::config::Config::load() {
                cfg.remember_password(&seed.target, Some(&seed.password));
                let _ = cfg.save();
            }
        }
        match self.hold(link, seed, sid) {
            Ok(held) => self.install(held),
            Err(e) => {
                self.tab_order.retain(|id| *id != sid);
                self.show_toast(format!("Could not open that computer: {e:#}"));
                self.apply_tab_strip();
            }
        }
    }

    fn fail_link(&mut self, sid: u64, error: String, id: String, files: bool, from_store: bool) {
        self.connecting.retain(|(existing, _)| *existing != sid);
        self.tab_order.retain(|existing| *existing != sid);
        self.cancelled.insert(sid);
        let wrong = error.to_ascii_lowercase().contains("wrong password");
        if wrong && from_store && pjdesk_proto::is_valid_id(&id) {
            if let Ok(mut cfg) = crate::config::Config::load() {
                cfg.remember_password(&id, None);
                if let Some(entry) = cfg.address_book.iter_mut().find(|entry| entry.id == id) {
                    entry.password = None;
                }
                let _ = cfg.save();
            }
        }
        if wrong {
            crate::session_window::write_admit_result(&id, 3, files);
            self.show_toast("Wrong password");
        } else {
            crate::note_failure(&error);
            crate::session_window::write_admit_result(&id, 1, files);
            self.show_toast(error);
        }
        self.apply_tab_strip();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn finish_relink(&mut self, sid: u64, link: Link, gen: u64) {
        let current = self.link_gen.lock().unwrap().get(&sid).copied();
        if current != Some(gen) || self.resume_stop.lock().unwrap().contains(&sid) {
            link.connection.close(0u32.into(), b"cancelled");
            return;
        }
        self.relinking.remove(&sid);
        self.retries.remove(&sid);
        self.cover.remove(&sid);
        self.remember_route(sid, &link);
        let pair = if sid == self.sid {
            Some((self.seed.view, self.seed.display))
        } else {
            self.others.iter().find(|held| held.sid == sid).map(|held| (held.seed.view, held.seed.display))
        };
        let Some((view, display)) = pair else {
            link.connection.close(0u32.into(), b"tab closed");
            return;
        };
        match arm_link(&self.rt, link, &self.proxy, sid, view, display) {
            Ok(armed) => {
                self.graft(sid, armed);
                self.show_toast("Reconnected");
            }
            Err(e) => self.fail_relink(sid, format!("{e:#}"), gen),
        }
    }

    fn fail_relink(&mut self, sid: u64, error: String, gen: u64) {
        if self.link_gen.lock().unwrap().get(&sid).copied() != Some(gen) {
            return;
        }
        self.relinking.remove(&sid);
        if self.resume_stop.lock().unwrap().contains(&sid) {
            return;
        }
        if let Some(plain) = pjdesk_proto::failover::session_refusal(&error) {
            self.retries.remove(&sid);
            info!("Reconnect stopped: {plain}");
            self.show_cover(sid, SessionCover::Ended(plain.to_string()));
            return;
        }
        let prev = self.retries.get(&sid).map(|wait| wait.delay).unwrap_or(Duration::from_secs(1));
        let delay = pjdesk_proto::signal::next_reconnect_delay(prev);
        self.retries.insert(sid, RetryWait { next_at: Instant::now() + delay, delay });
        info!("Could not reconnect ({}); trying again in {} s", public_failure(&error), delay.as_secs());
        self.show_cover(sid, SessionCover::Reconnecting);
    }

    fn schedule_retry(&mut self, sid: u64) {
        if self.resume_stop.lock().unwrap().contains(&sid) {
            return;
        }
        if self.relinking.contains(&sid) {
            self.show_cover(sid, SessionCover::Reconnecting);
            return;
        }
        self.retries.entry(sid).or_insert(RetryWait { next_at: Instant::now(), delay: Duration::from_secs(1) });
        self.begin_relink(sid, String::new(), false);
    }

    fn cover_detail(&self, sid: u64, cover: &SessionCover) -> String {
        match cover {
            SessionCover::Reconnecting => {
                let trying = self.relinking.contains(&sid);
                let secs = self
                    .retries
                    .get(&sid)
                    .and_then(|wait| wait.next_at.checked_duration_since(Instant::now()))
                    .map(|left| left.as_secs())
                    .unwrap_or(0);
                pjdesk_proto::failover::retry_countdown(secs, trying)
            }
            SessionCover::Lost => String::new(),
            SessionCover::Ended(text) => text.clone(),
        }
    }

    fn refuse_session(&mut self, sid: u64, plain: &str) {
        self.resume_stop.lock().unwrap().insert(sid);
        self.relinking.remove(&sid);
        self.retries.remove(&sid);
        info!("Session refused: {plain}");
        self.show_cover(sid, SessionCover::Ended(plain.to_string()));
    }

    fn remember_route(&mut self, sid: u64, link: &Link) {
        let apply = |seed: &mut Seed| {
            if !link.direct_targets.is_empty() {
                seed.direct_targets = link.direct_targets.clone();
            }
            if let Some(fp) = link.host_fingerprint {
                seed.host_fingerprint = Some(fp);
            }
        };
        if sid == self.sid {
            apply(&mut self.seed);
        } else if let Some(held) = self.others.iter_mut().find(|held| held.sid == sid) {
            apply(&mut held.seed);
        }
    }

    fn hold(&self, link: Link, mut seed: Seed, sid: u64) -> Result<Held> {
        if !link.direct_targets.is_empty() {
            seed.direct_targets = link.direct_targets.clone();
        }
        if link.host_fingerprint.is_some() {
            seed.host_fingerprint = link.host_fingerprint;
        }
        let armed = arm_link(&self.rt, link, &self.proxy, sid, seed.view, seed.display)?;
        let places = crate::config::Config::load()
            .ok()
            .and_then(|cfg| cfg.settings.peers.get(&armed.peer_id).map(|note| note.windows.clone()))
            .unwrap_or_default();
        let mut toolbar = super::toolbar::Toolbar::default();
        toolbar.set_light(self.light);
        let mut panel = super::panel::Panel::default();
        if seed.files {
            panel.open = true;
            panel.tab = 0;
        }
        Ok(Held {
            sid,
            primary_display: seed.display,
            seed,
            title: armed.title,
            slot: armed.slot,
            current: None,
            layout: Layout::default(),
            control: armed.control,
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
            live: armed.live,
            cmds: armed.cmds,
            panel,
            restarting: false,
            relay_resume: false,
            relay_primary: String::new(),
            peer_id: armed.peer_id,
            host_name: armed.host_name,
            extra: Vec::new(),
            toast: None,
            sent_watch: Vec::new(),
            places,
            place_saved: Instant::now() - Duration::from_secs(5),
            placed_primary: false,
            noted_platform: String::new(),
            remote_size: None,
            fitted_extra: None,
            android_stroke: pjdesk_proto::gesture::StrokeTracker::default(),
            pointer_down: false,
            saw_picture: armed.saw_picture,
            connection: armed.connection,
            endpoint: armed.endpoint,
            retire: armed.retire,
        })
    }

    fn graft(&mut self, sid: u64, armed: Armed) {
        if sid == self.sid {
            self.connection.close(0u32.into(), b"reconnecting");
            self.connection = armed.connection;
            self.endpoint = armed.endpoint;
            self.slot = armed.slot;
            self.control = armed.control;
            self.live = armed.live;
            self.cmds = armed.cmds;
            self.saw_picture = armed.saw_picture;
            self.retire = armed.retire;
            self.closed = false;
            self.restarting = false;
            self.relay_resume = false;
            self.extra.clear();
            self.host_name = armed.host_name;
            self.title = armed.title;
            self.peer_id = armed.peer_id;
            self.sized_to_remote = false;
            self.remote_size = None;
        } else if let Some(held) = self.others.iter_mut().find(|held| held.sid == sid) {
            held.connection.close(0u32.into(), b"reconnecting");
            held.connection = armed.connection;
            held.endpoint = armed.endpoint;
            held.slot = armed.slot;
            held.control = armed.control;
            held.live = armed.live;
            held.cmds = armed.cmds;
            held.saw_picture = armed.saw_picture;
            held.retire = armed.retire;
            held.closed = false;
            held.restarting = false;
            held.relay_resume = false;
            held.extra.clear();
            held.host_name = armed.host_name;
            held.title = armed.title;
            held.peer_id = armed.peer_id;
            held.sized_to_remote = false;
            held.remote_size = None;
        }
        self.apply_tab_strip();
        self.reveal();
    }
}

fn hide_panes(extra: &mut [ExtraPane]) {
    for pane in extra.iter() {
        pane.window.set_visible(false);
    }
}

fn show_panes(extra: &[ExtraPane]) {
    for pane in extra {
        pane.window.set_visible(true);
        pane.window.request_redraw();
    }
}

fn session_name(host_name: &str, peer_id: &str, label: &str) -> String {
    let host_name = host_name.trim();
    if !host_name.is_empty() {
        return host_name.to_string();
    }
    if !label.trim().is_empty() {
        return label.trim().to_string();
    }
    if pjdesk_proto::is_valid_id(peer_id) {
        return format_id(peer_id);
    }
    "Session".into()
}

fn seed_from_admit(admit: &crate::session_window::Admit) -> Seed {
    Seed {
        server: admit.server.clone(),
        target: admit.id.clone(),
        password: admit.password.clone(),
        direct: if admit.direct.trim().is_empty() { None } else { Some(admit.direct.clone()) },
        force_relay: admit.force_relay,
        display: admit.display,
        view: admit.view,
        remember: admit.remember,
        from_store: admit.from_store,
        files: admit.files,
        label: admit.label.clone(),
        direct_targets: Vec::new(),
        host_fingerprint: None,
    }
}

fn cover_kind(cover: &SessionCover) -> toolbar::SessionCover {
    match cover {
        SessionCover::Reconnecting => toolbar::SessionCover::Reconnecting,
        SessionCover::Lost => toolbar::SessionCover::Lost,
        SessionCover::Ended(_) => toolbar::SessionCover::Ended,
    }
}

/// A short line for the log. A password, token, or PIN is not included.
fn public_failure(error: &str) -> String {
    let line = error.lines().next().unwrap_or("").trim();
    let lower = line.to_ascii_lowercase();
    if lower.contains("password") || lower.contains("token") || lower.contains("pin") {
        return "the other computer refused the session".into();
    }
    line.chars().take(180).collect()
}

fn admit_from_seed(seed: &Seed) -> crate::session_window::Admit {
    crate::session_window::Admit {
        id: seed.target.clone(),
        password: seed.password.clone(),
        server: seed.server.clone(),
        direct: seed.direct.clone().unwrap_or_default(),
        label: seed.label.clone(),
        files: seed.files,
        remember: seed.remember,
        from_store: seed.from_store,
        force_relay: seed.force_relay,
        view: seed.view,
        display: seed.display,
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // No system title bar: the computer and screen tabs sit in PJDesk's
        // own strip with minimize, maximize and close (see toolbar::TITLE_H).
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(PhysicalSize::new(1280u32, 720u32))
            .with_decorations(false)
            .with_visible(true)
            .with_active(true);
        #[cfg(windows)]
        let attributes = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes.with_undecorated_shadow(true)
        };
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(e) => {
                let text = format!("Could not open the viewer window: {e}");
                crate::note_failure(&text);
                info!("{text}");
                self.failed_open = Some(text);
                event_loop.exit();
                return;
            }
        };
        info!("Viewer window is open");
        self.hook_drops(&window);
        let waiting = self
            .live
            .lock()
            .unwrap()
            .notices
            .iter()
            .rev()
            .find(|note| super::controls::screen_share_waiting(note))
            .cloned();
        if let Some(text) = waiting {
            crate::note_status(&text);
        } else {
            crate::note_status("Viewer window is open. Waiting for the picture.");
        }
        self.window_shown.store(true, Ordering::SeqCst);
        window.focus_window();
        window.request_user_attention(Some(UserAttentionType::Critical));
        if let Ok(source) = CustomCursor::from_rgba(dot_cursor(), 5, 5, 2, 2) {
            window.set_cursor(event_loop.create_custom_cursor(source));
        }
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        match surface {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => {
                let text = format!("Could not draw the viewer window: {e}");
                crate::note_failure(&text);
                info!("{text}");
                self.failed_open = Some(text);
                event_loop.exit();
                return;
            }
        }
        if self.follow_system_theme {
            self.light = matches!(window.theme(), Some(winit::window::Theme::Light));
            self.toolbar.set_light(self.light);
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Work => {
                self.drain_queue();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            UserEvent::Frame(session) => {
                if self.note_transport(event_loop, session) {
                    return;
                }
                if session != self.sid {
                    return;
                }
                let Some(window) = self.window.clone() else { return };
                self.sync_separate(event_loop);
                window.request_redraw();
                for pane in &self.extra {
                    pane.window.request_redraw();
                }
            }
            UserEvent::Title { session, text } => {
                if session != self.sid || self.closed {
                    return;
                }
                let Some(window) = self.window.clone() else { return };
                if self.is_separate() {
                    window.set_title(&self.screen_named(self.primary_display));
                    for pane in &self.extra {
                        pane.window.set_title(&self.screen_named(pane.display));
                    }
                } else {
                    window.set_title(&text);
                }
            }
            UserEvent::Files(paths) => self.accept_dropped(paths),
            UserEvent::Closed { session, reason } => {
                if self.relay_or_restart(event_loop, session, &reason) {
                    return;
                }
                if self.resume_stop.lock().unwrap().contains(&session) {
                    return;
                }
                if self.transport_flags(session).is_some_and(|(_, _, _, ended)| ended.is_some()) {
                    let plain = pjdesk_proto::failover::session_refusal(&reason)
                        .unwrap_or("The other computer ended the session.");
                    self.refuse_session(session, plain);
                    return;
                }
                if let Some(plain) = pjdesk_proto::failover::session_refusal(&reason) {
                    self.refuse_session(session, plain);
                    return;
                }
                info!("Connection lost ({})", public_failure(&reason));
                crate::note_status("Connection lost, reconnecting…");
                self.schedule_retry(session);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.retries.is_empty() {
            self.retry_drawn = None;
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            return;
        }
        let now = Instant::now();
        let due: Vec<u64> = self
            .retries
            .iter()
            .filter(|(sid, wait)| {
                wait.next_at <= now
                    && !self.relinking.contains(sid)
                    && matches!(self.cover.get(sid), Some(SessionCover::Reconnecting))
            })
            .map(|(sid, _)| *sid)
            .collect();
        let starting = !due.is_empty();
        for sid in due {
            self.begin_relink(sid, String::new(), false);
        }
        let secs_now = self
            .retries
            .values()
            .map(|wait| wait.next_at.checked_duration_since(now).map(|left| left.as_secs()).unwrap_or(0))
            .min();
        if starting || self.retry_drawn != secs_now {
            self.retry_drawn = secs_now;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            for pane in &self.extra {
                pane.window.request_redraw();
            }
        }
        let soon = self.retries.values().map(|wait| wait.next_at).min();
        if let Some(at) = soon {
            let tick = Instant::now() + Duration::from_secs(1);
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(at.min(tick)));
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
            WindowEvent::CloseRequested => self.request_close(event_loop),
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
                    self.light = matches!(theme, winit::window::Theme::Light);
                    self.toolbar.set_light(self.light);
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
                if state == ElementState::Pressed
                    && button == winit::event::MouseButton::Left
                    && self.title_press(event_loop)
                {
                    return;
                }
                let win_w = self.window.as_ref().map(|w| w.inner_size().width as usize).unwrap_or(0);
                let win_h = self.window.as_ref().map(|w| w.inner_size().height as usize).unwrap_or(0);
                if self.confirm_close {
                    if state == ElementState::Pressed && button == winit::event::MouseButton::Left {
                        if let Some(close_all) = toolbar::confirm_close_hit(self.cursor_px.0, self.cursor_px.1, win_w, win_h)
                        {
                            if close_all {
                                event_loop.exit();
                            } else {
                                self.confirm_close = false;
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                            }
                        }
                    }
                    return;
                }
                if let Some(cover) = self.cover.get(&self.sid).cloned() {
                    if state == ElementState::Pressed && button == winit::event::MouseButton::Left {
                        let kind = match cover {
                            SessionCover::Reconnecting => toolbar::SessionCover::Reconnecting,
                            SessionCover::Lost => toolbar::SessionCover::Lost,
                            SessionCover::Ended(_) => toolbar::SessionCover::Ended,
                        };
                        if let Some(hit) = toolbar::session_cover_hit(kind, self.cursor_px.0, self.cursor_px.1, win_w, win_h) {
                            let sid = self.sid;
                            match hit {
                                toolbar::SessionCoverAction::Cancel => self.stop_retry(sid),
                                toolbar::SessionCoverAction::Reconnect => self.reconnect_now(sid),
                                toolbar::SessionCoverAction::Close => {
                                    if self.end_sid(sid) {
                                        event_loop.exit();
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
                if self.toolbar.shows_android_nav() {
                    let disabled = self.live.lock().unwrap().control_off;
                    let lay = self.layout;
                    if let Some(hit) = toolbar::android_bar_hit(
                        self.cursor_px.0,
                        self.cursor_px.1,
                        lay.x,
                        lay.y,
                        lay.width,
                        lay.height,
                        win_w,
                        win_h,
                        disabled,
                    ) {
                        if state == ElementState::Pressed {
                            if let toolbar::AndroidHit::Button(which) = hit {
                                if !disabled {
                                    self.send_android(which);
                                }
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        } else if self.pointer_down {
                            self.on_android_button(false);
                        }
                        return;
                    }
                }
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
                            if self.panel.tab == 1 && !self.live.lock().unwrap().chat_ready {
                                self.panel.tab = 0;
                                self.live.lock().unwrap().notices.push(pjdesk_proto::peer::CHAT_NEEDS_TIP.into());
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
                    } else if button == MouseButton::Right && state == ElementState::Pressed {
                        if let Some(point) = self.pointer_norm() {
                            self.send(pjdesk_proto::gesture::Gesture::long_press(point.0, point.1, 500).to_event());
                        }
                    }
                } else {
                    self.send(InputEvent::MouseButton { button, down: state == ElementState::Pressed });
                }
            }
            WindowEvent::DroppedFile(path) => self.accept_dropped(vec![path]),
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

/// The resize border of a window without a system frame: about 6 px, a
/// little more at the corners.
fn resize_edge(px: f64, py: f64, w: f64, h: f64, scale: f64) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection as D;
    let band = 6.0 * scale.max(1.0);
    let corner = band * 2.0;
    if px < 0.0 || py < 0.0 || px > w || py > h {
        return None;
    }
    let (l, r, t, b) = (px < band, px > w - band, py < band, py > h - band);
    let (lc, rc, tc, bc) = (px < corner, px > w - corner, py < corner, py > h - corner);
    Some(match () {
        _ if (t && lc) || (l && tc) => D::NorthWest,
        _ if (t && rc) || (r && tc) => D::NorthEast,
        _ if (b && lc) || (l && bc) => D::SouthWest,
        _ if (b && rc) || (r && bc) => D::SouthEast,
        _ if t => D::North,
        _ if b => D::South,
        _ if l => D::West,
        _ if r => D::East,
        _ => return None,
    })
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

/// Esc is Back, Home is Home, the menu key is Recents. Volume and power keys follow.
pub(crate) fn android_nav_key(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Escape | KeyCode::BrowserBack => 0,
        KeyCode::Home | KeyCode::BrowserHome => 1,
        KeyCode::ContextMenu => 2,
        KeyCode::AudioVolumeUp => 3,
        KeyCode::AudioVolumeDown => 4,
        KeyCode::Power => 5,
        _ => return None,
    })
}

/// Logical keys that do not have their own physical code: the Android app-switch
/// and the phone Back/Home keys.
pub(crate) fn android_nav_named(key: &Key) -> Option<u8> {
    let Key::Named(named) = key else { return None };
    Some(match named {
        winit::keyboard::NamedKey::GoBack => 0,
        winit::keyboard::NamedKey::GoHome => 1,
        winit::keyboard::NamedKey::AppSwitch => 2,
        _ => return None,
    })
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

/// A dropped file is sent as itself. A dropped folder is the files inside it.
fn expand_drop(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    walk_drop(path, 0, &mut out);
    out.truncate(200);
    out
}

fn walk_drop(path: &std::path::Path, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    if out.len() >= 200 || depth > 6 {
        return;
    }
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    if !path.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        walk_drop(&entry.path(), depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{android_nav_key, android_nav_named};
    use winit::keyboard::{Key, KeyCode, NamedKey};

    #[test]
    fn android_keyboard_shortcuts_match_the_nav_bar() {
        assert_eq!(android_nav_key(KeyCode::Escape), Some(0));
        assert_eq!(android_nav_key(KeyCode::BrowserBack), Some(0));
        assert_eq!(android_nav_key(KeyCode::Home), Some(1));
        assert_eq!(android_nav_key(KeyCode::BrowserHome), Some(1));
        assert_eq!(android_nav_key(KeyCode::ContextMenu), Some(2));
        assert_eq!(android_nav_key(KeyCode::AudioVolumeUp), Some(3));
        assert_eq!(android_nav_key(KeyCode::AudioVolumeDown), Some(4));
        assert_eq!(android_nav_key(KeyCode::Power), Some(5));
        assert_eq!(android_nav_key(KeyCode::KeyA), None);
        assert_eq!(android_nav_key(KeyCode::Enter), None);
        assert_eq!(android_nav_named(&Key::Named(NamedKey::AppSwitch)), Some(2));
        assert_eq!(android_nav_named(&Key::Named(NamedKey::GoBack)), Some(0));
        assert_eq!(android_nav_named(&Key::Named(NamedKey::GoHome)), Some(1));
        assert_eq!(android_nav_named(&Key::Named(NamedKey::Home)), None);
    }
}
