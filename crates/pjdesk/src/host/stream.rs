//! The streaming part of a host session.
//!
//! ```text
//!  capture thread                         async tasks
//!  ┌───────────────────────────┐   frames  ┌──────────────┐   QUIC uni stream
//!  │ wait for credit (≤3 in    │ ────────► │ video sender │ ─────────────────► controller
//!  │ flight) → capture → BGRA  │           └──────────────┘
//!  │ → I420 → H.264            │           ┌──────────────┐   QUIC bi stream
//!  └───────────────────────────┘ ◄──acks── │ control loop │ ◄───────────────── input, acks
//!                                          └──────┬───────┘
//!                                                 └─► SendInput
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pjdesk_proto::framing::{write_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::peer::{write_video_frame, ControlMsg, HostMsg, VideoHeader};
use quinn::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::capture::{self, Rect};
use super::ctrl::{Grant, Session};
use super::encoder::VideoEncoder;
use super::input::InputInjector;
use crate::color::{scaled_size, I420Buffer};
use crate::host::HostShared;
use crate::HostArgs;

/// Frames sent but not yet decoded by the controller. Small = low latency.
const MAX_IN_FLIGHT: u32 = 3;

struct EncodedFrame {
    header: VideoHeader,
    data: Vec<u8>,
}

/// Credit-based flow control between the capture thread and the controller's acks.
struct Flow {
    state: Mutex<FlowState>,
    cond: Condvar,
}

struct FlowState {
    sent: u32,
    acked: u32,
    stopped: bool,
}

impl Flow {
    fn new() -> Self {
        Self { state: Mutex::new(FlowState { sent: 0, acked: 0, stopped: false }), cond: Condvar::new() }
    }

    /// Waits until another frame may be sent. False on timeout or stop.
    fn wait_for_credit(&self, limit: Duration) -> bool {
        let guard = self.state.lock().unwrap();
        let (guard, _) = self
            .cond
            .wait_timeout_while(guard, limit, |s| !s.stopped && s.sent.wrapping_sub(s.acked) >= MAX_IN_FLIGHT)
            .unwrap();
        !guard.stopped && guard.sent.wrapping_sub(guard.acked) < MAX_IN_FLIGHT
    }

    fn sent(&self, seq: u32) {
        self.state.lock().unwrap().sent = seq;
    }

    fn ack(&self, seq: u32) {
        let mut state = self.state.lock().unwrap();
        // Ignore stale or bogus acks (sequence numbers only move forward).
        if (seq.wrapping_sub(state.acked) as i32) > 0 && (state.sent.wrapping_sub(seq) as i32) >= 0 {
            state.acked = seq;
            self.cond.notify_all();
        }
    }

    fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
        self.cond.notify_all();
    }

    fn stopped(&self) -> bool {
        self.state.lock().unwrap().stopped
    }
}

struct Shared {
    flow: Flow,
    keyframe_requested: AtomicBool,
    display: Mutex<Rect>,
}

/// Streams the screen and replays input until the controller disconnects.
pub async fn run(
    connection: Connection,
    mut control_send: SendStream,
    mut control_recv: RecvStream,
    args: HostArgs,
    grant: Grant,
    ui: Option<Arc<HostShared>>,
    first: Option<ControlMsg>,
) -> Result<()> {
    let shared = Arc::new(Shared {
        flow: Flow::new(),
        keyframe_requested: AtomicBool::new(false),
        display: Mutex::new(Rect::default()),
    });

    let (frame_tx, mut frame_rx) = mpsc::channel::<EncodedFrame>(MAX_IN_FLIGHT as usize + 1);
    let (host_tx, mut host_rx) = mpsc::unbounded_channel::<HostMsg>();
    let peer_name = grant.peer_name.clone();
    let session = Session::new(
        args.fps,
        args.bitrate,
        args.max_width,
        args.display,
        args.view_only,
        args.service_host,
        grant,
        connection.clone(),
        host_tx,
        ui.clone(),
    );
    // A normal session always owns the host's files window and notice.
    let _lease = UiLease(super::ctrl::claim_ui(true).unwrap_or(0));
    session.ui_gen.store(_lease.0, Ordering::SeqCst);
    session.spawn_side_loops();
    let _notice = if args.service_host {
        super::notice::Guard::begin_service(&peer_name)
    } else {
        super::notice::Guard::begin(&peer_name)
    };
    let handed = args.service_host && super::notice::client_alive();
    let _chat = super::chatwin::Guard::begin(handed);
    // Small minimized taskbar window with Chat and Send files; closes with the session.
    let _files = super::filewin::Guard::begin(&peer_name);
    if let Some(shared) = &ui {
        shared.clear_chat();
    }

    let (cap_err_tx, mut cap_err_rx) = mpsc::channel::<String>(1);
    let capture_thread = {
        let shared = shared.clone();
        let session = session.clone();
        let args = args.clone();
        std::thread::Builder::new().name("capture".into()).spawn(move || {
            if let Err(e) = capture_loop(&args, &shared, &session, &frame_tx) {
                warn!("Screen capture stopped: {e:#}");
                let _ = cap_err_tx.blocking_send(format!("{e:#}"));
            }
        })?
    };

    info!("Opening the video stream to the controller");
    let mut video = connection.open_uni().await.context("could not open the video stream")?;
    info!("Video stream is open");
    // Input and acks on the control stream go before video bytes.
    let _ = video.set_priority(-1);
    let mut video_task = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            write_video_frame(&mut video, &frame.header, &frame.data).await?;
        }
        let _ = video.finish();
        Ok::<_, anyhow::Error>(())
    });

    let (mouse, keyboard) = session.initial_mouse_keyboard();
    let mut injector = InputInjector::new(!mouse && !keyboard);
    injector.set_allowed(mouse, keyboard);
    if let Some(msg) = first {
        let display = *shared.display.lock().unwrap();
        if session.handle(msg, &mut control_send, &mut injector, display, &shared.keyframe_requested).await? {
            video_task.abort();
            shared.flow.stop();
            let _ = tokio::task::spawn_blocking(move || capture_thread.join()).await;
            return Ok(());
        }
    }
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    // Keeps a partly read message when another branch of the select wins.
    let mut frames = pjdesk_proto::framing::FrameReader::new();
    let result = loop {
        tokio::select! {
            msg = frames.read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME) => match msg {
                Ok(msg) => {
                    if let ControlMsg::FrameAck { seq } = &msg {
                        shared.flow.ack(*seq);
                    }
                    let display = *shared.display.lock().unwrap();
                    match session.handle(msg, &mut control_send, &mut injector, display, &shared.keyframe_requested).await {
                        Ok(true) => break Ok(()),
                        Ok(false) => {}
                        Err(e) => break Err(e),
                    }
                }
                Err(_) => break Ok(()),
            },
            Some(out) = host_rx.recv() => {
                if let Err(e) = write_msg(&mut control_send, &out).await {
                    break Err(e);
                }
            }
            _ = tick.tick() => {
                if let Some(ui) = &ui {
                    let primary = ui.relay_move.lock().unwrap().clone();
                    if let Some(primary) = primary {
                        if !ui.relay_sent.load(Ordering::SeqCst) {
                            info!("failover: telling the controller to reconnect through the primary {primary}");
                            let _ = write_msg(&mut control_send, &HostMsg::MoveRelay { primary }).await;
                            ui.relay_sent.store(true, Ordering::SeqCst);
                            *ui.relay_move.lock().unwrap() = None;
                        }
                    }
                }
                if super::notice::disconnect_requested() {
                    info!("The person at this computer disconnected the session");
                    let _ = write_msg(&mut control_send, &HostMsg::Notice("The person at this computer disconnected".into())).await;
                    break Ok(());
                }
                // Input self-check: tell the viewer once when moves stop landing.
                if let Some(problem) = super::inputroute::take_problem() {
                    let _ = write_msg(&mut control_send, &HostMsg::Notice(problem)).await;
                }
                if let Err(e) = session.poll_local(&mut control_send).await {
                    break Err(e);
                }
            }
            done = &mut video_task => break match done {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e).context("sending video"),
                Err(e) => Err(e.into()),
            },
            Some(err) = cap_err_rx.recv() => {
                let message = format!("Screen capture failed: {err}");
                warn!("{message}");
                let _ = write_msg(&mut control_send, &HostMsg::Notice(message.clone())).await;
                break Err(anyhow::anyhow!(message));
            }
            _ = connection.closed() => break Ok(()),
        }
    };

    shared.flow.stop();
    session.release_privacy();
    injector.release_all();
    video_task.abort();
    let _ = tokio::task::spawn_blocking(move || capture_thread.join()).await;
    result
}

/// Starts an authenticated session: files-only when the controller's first
/// message asks for it, otherwise the normal picture session.
pub async fn begin(
    connection: Connection,
    control_send: SendStream,
    mut control_recv: RecvStream,
    args: HostArgs,
    grant: Grant,
    ui: Option<Arc<HostShared>>,
) -> Result<()> {
    let first = if grant.peer_proto >= 3 {
        // Version-3 controllers send Hello at once. Exact reads: nothing is read ahead.
        tokio::time::timeout(
            Duration::from_secs(10),
            pjdesk_proto::framing::read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME),
        )
        .await
        .ok()
        .and_then(|m| m.ok())
    } else {
        None
    };
    match first {
        Some(hello) if wants_files_only(&hello) => {
            if let Some(shared) = &ui {
                *shared.connected.lock().unwrap() = Some(files_only_label(&grant.peer_name));
            }
            run_files(connection, control_send, control_recv, args, grant, ui, hello).await
        }
        first => run(connection, control_send, control_recv, args, grant, ui, first).await,
    }
}

pub fn wants_files_only(msg: &ControlMsg) -> bool {
    matches!(msg, ControlMsg::Hello { permissions, .. } if permissions & pjdesk_proto::perms::FILES_ONLY != 0)
}

/// Releases the claim on the host's files window when a session ends.
struct UiLease(u64);

impl Drop for UiLease {
    fn drop(&mut self) {
        super::ctrl::release_ui(self.0);
    }
}

/// The popup label for a files-only session.
pub fn files_only_label(peer: &str) -> String {
    let peer = peer.trim();
    if peer.is_empty() {
        "File transfer".to_string()
    } else {
        format!("{peer} (file transfer)")
    }
}

/// A files-only session: the same authentication as a normal one, then only the
/// control stream and file side streams. No capture, encoder, video or input.
/// It does not take the single-session slot, and when another session is on it
/// leaves that session's notice and files window alone.
pub async fn run_files(
    connection: Connection,
    mut control_send: SendStream,
    mut control_recv: RecvStream,
    args: HostArgs,
    grant: Grant,
    ui: Option<Arc<HostShared>>,
    hello: ControlMsg,
) -> Result<()> {
    let (host_tx, mut host_rx) = mpsc::unbounded_channel::<HostMsg>();
    let label = files_only_label(&grant.peer_name);
    let session = Session::new(
        args.fps,
        args.bitrate,
        args.max_width,
        args.display,
        args.view_only,
        args.service_host,
        grant,
        connection.clone(),
        host_tx,
        ui.clone(),
    );
    session.files_only.store(true, Ordering::SeqCst);
    let gen = super::ctrl::claim_ui(false);
    let _lease = UiLease(gen.unwrap_or(0));
    session.ui_gen.store(_lease.0, Ordering::SeqCst);
    session.spawn_side_loops();
    // Only the owner shows the popup and the files window (receive view).
    let mut windows = gen.map(|_| {
        let notice = if args.service_host {
            super::notice::Guard::begin_service(&label)
        } else {
            super::notice::Guard::begin(&label)
        };
        let files = super::filewin::Guard::begin(&label);
        super::filewin::show_receive();
        (notice, files)
    });
    info!("Files-only session for {label}{}", if gen.is_none() { " alongside another session" } else { "" });

    let mut injector = InputInjector::new(true);
    injector.set_allowed(false, false);
    let keyframe = AtomicBool::new(false);
    let display = Rect::default();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let mut frames = pjdesk_proto::framing::FrameReader::new();
    let mut result = match session.handle(hello, &mut control_send, &mut injector, display, &keyframe).await {
        Ok(true) => Some(Ok(())),
        Ok(false) => None,
        Err(e) => Some(Err(e)),
    };
    while result.is_none() {
        tokio::select! {
            msg = frames.read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME) => match msg {
                Ok(msg) => match session.handle(msg, &mut control_send, &mut injector, display, &keyframe).await {
                    Ok(true) => result = Some(Ok(())),
                    Ok(false) => {}
                    Err(e) => result = Some(Err(e)),
                },
                Err(_) => result = Some(Ok(())),
            },
            Some(out) = host_rx.recv() => {
                if let Err(e) = write_msg(&mut control_send, &out).await {
                    result = Some(Err(e));
                }
            }
            _ = tick.tick() => {
                let owner = super::ctrl::owns_ui(_lease.0);
                if owner && super::notice::disconnect_requested() {
                    info!("The person at this computer ended the file transfer");
                    let _ = write_msg(&mut control_send, &HostMsg::Notice("The person at this computer disconnected".into())).await;
                    result = Some(Ok(()));
                } else if let Err(e) = session.poll_local(&mut control_send).await {
                    result = Some(Err(e));
                }
            }
            _ = connection.closed() => result = Some(Ok(())),
        }
    }
    injector.release_all();
    // A normal session that started meanwhile took the windows; leave them up.
    if !super::ctrl::owns_ui(_lease.0) {
        if let Some(w) = windows.take() {
            std::mem::forget(w);
        }
    }
    drop(windows);
    result.unwrap_or(Ok(()))
}

#[cfg(test)]
mod files_only_label_tests {
    #[test]
    fn the_popup_names_a_file_transfer() {
        assert_eq!(super::files_only_label("Ben's laptop"), "Ben's laptop (file transfer)");
        assert_eq!(super::files_only_label(" "), "File transfer");
        let hello =
            |p| pjdesk_proto::peer::ControlMsg::Hello { permissions: p, monitor: 0, quality: Default::default() };
        assert!(super::wants_files_only(&hello(pjdesk_proto::perms::FILES_ONLY_REQUEST)));
        assert!(!super::wants_files_only(&hello(pjdesk_proto::perms::ALL)));
    }
}

/// Integer downscale factor that keeps the width within `max_width` and the
/// picture within OpenH264's 3840×2160 limit.
fn scale_factor(width: usize, height: usize, max_width: usize) -> usize {
    let mut factor = width.div_ceil(max_width.max(320)).max(1);
    loop {
        let (w, h) = scaled_size(width, height, factor);
        let (long, short) = (w.max(h), w.min(h));
        if long <= 3840 && short <= 2160 {
            return factor;
        }
        factor += 1;
    }
}

/// Capture → convert → encode loop (runs on its own thread).
fn capture_loop(args: &HostArgs, shared: &Shared, session: &Session, frames: &mpsc::Sender<EncodedFrame>) -> Result<()> {
    let _awake = KeepAwake::new();
    match crate::platform::session_id() {
        Some(0) => warn!(
            "Screen capture is running in Windows session 0, which has no desktop. The PJDesk service launches a helper in the signed-in session; this process is not that helper."
        ),
        Some(id) => info!("Screen capture is in Windows session {id}"),
        None => {}
    }
    // Service helper: attach this capture thread to the input desktop so it can
    // see UAC prompts, the lock screen and the sign-in screen.
    let mut desktop = String::new();
    if args.follow_desktop {
        if crate::platform::attach_thread_to_input_desktop() {
            desktop = crate::platform::input_desktop_name().unwrap_or_default();
            info!("Capture thread is on the input desktop \"{desktop}\"");
        } else {
            warn!("Could not attach capture to the input desktop. The picture may stay black.");
            session.notify("Could not attach to the desktop that is on screen. The picture may stay black.");
        }
    }
    let mut capturer = capture::open(args)?;
    info!("Capturing {} with {}", describe(capturer.display_rect()), capturer.name());
    *shared.display.lock().unwrap() = capturer.display_rect();

    let mut facts_checked = Instant::now();
    let mut facts_problem: Option<String> = SessionFacts::now().problem();
    if let Some(p) = &facts_problem {
        warn!("Session state: {p}");
        if let Some(state) = SessionFacts::now().state() {
            session.notify(&pjdesk_proto::sessionnote::text(&crate::hostname(), state));
        }
    }
    let mut live_fps = session.fps.load(Ordering::SeqCst);
    let mut live_bitrate = session.bitrate.load(Ordering::SeqCst);
    let mut encoder = VideoEncoder::new(live_bitrate, live_fps)?;
    let mut interval = Duration::from_secs_f64(1.0 / live_fps.clamp(1, 120) as f64);
    let mut next_due = Instant::now();
    let mut seq = 0u32;
    let mut picture = I420Buffer::default();
    let mut errors = 0u32;
    let mut idle_misses = 0u32;
    let mut refine = pjdesk_proto::quality::Refine::default();
    // The newest image in `picture` hasn't reached the controller yet
    // (the encoder dropped it to hold the bitrate).
    let mut undelivered = false;
    // When GDI is only a fallback, keep trying to get back to DXGI
    // (it may have been unavailable at start, e.g. during a UAC prompt).
    let mut next_dxgi_retry = Instant::now() + Duration::from_secs(10);
    let mut args = args.clone();
    let mut plan_watch: Vec<u8> = Vec::new();
    let mut plan_layout = String::new();
    let mut black_since: Option<Instant> = None;
    let mut black_told = false;

    while !shared.flow.stopped() {
        let (want_display, want_watch, want_layout) = session.capture_plan();
        if want_display != args.display || want_watch != plan_watch || want_layout != plan_layout {
            let label = if want_display == capture::SEPARATE_DISPLAYS {
                format!("displays {want_watch:?}")
            } else if want_display == capture::ALL_DISPLAYS {
                "all displays".to_string()
            } else {
                format!("monitor {want_display}")
            };
            let opened = if want_display == capture::SEPARATE_DISPLAYS {
                if want_watch.is_empty() {
                    None
                } else {
                    match capture::open_separate(&want_watch) {
                        Ok(next) => Some(next),
                        Err(e) => {
                            warn!("Could not switch to {label} ({e:#})");
                            None
                        }
                    }
                }
            } else {
                let mut next_args = args.clone();
                next_args.display = want_display;
                match capture::open(&next_args) {
                    Ok(next) => Some(next),
                    Err(e) => {
                        warn!("Could not switch to {label} ({e:#})");
                        None
                    }
                }
            };
            match opened {
                Some(next) => {
                    info!("Switched to {label}");
                    args.display = want_display;
                    plan_watch = want_watch;
                    plan_layout = want_layout;
                    if let Some((w, h, tiles)) = next.atlas() {
                        session.note_atlas(w, h, tiles);
                    }
                    capturer = next;
                    *shared.display.lock().unwrap() = capturer.display_rect();
                    shared.keyframe_requested.store(true, Ordering::SeqCst);
                }
                None if want_display == capture::SEPARATE_DISPLAYS && want_watch.is_empty() => {
                    args.display = want_display;
                    plan_watch = want_watch;
                    plan_layout = want_layout;
                }
                None => {}
            }
        }
        if args.display == capture::SEPARATE_DISPLAYS && plan_watch.is_empty() {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let fps_now = session.fps.load(Ordering::SeqCst).clamp(1, 120);
        let bitrate_now = session.bitrate.load(Ordering::SeqCst).clamp(200, 50_000);
        if fps_now != live_fps || bitrate_now.abs_diff(live_bitrate) > live_bitrate / 10 {
            live_fps = fps_now;
            live_bitrate = bitrate_now;
            encoder = VideoEncoder::new(live_bitrate, live_fps)?;
            interval = Duration::from_secs_f64(1.0 / live_fps as f64);
            shared.keyframe_requested.store(true, Ordering::SeqCst);
        }
        #[cfg(windows)]
        capture::set_draw_pointer(!session.cursor_shapes.load(Ordering::Relaxed));
        // Follow desktop switches (UAC / lock / sign-in screen): re-attach this
        // thread and rebuild capture so it grabs the desktop now on screen.
        // An empty name means the read failed transiently — don't treat that as
        // a switch, and don't let a transient capture failure kill the thread.
        if args.follow_desktop {
            let current = crate::platform::input_desktop_name().unwrap_or_default();
            if !current.is_empty() && current != desktop {
                info!("Input desktop changed to \"{current}\"; following it");
                desktop = current;
                crate::platform::attach_thread_to_input_desktop();
                drop(capturer);
                std::thread::sleep(Duration::from_millis(200));
                capturer = loop {
                    match capture::open(&args) {
                        Ok(c) => break c,
                        Err(e) => {
                            if shared.flow.stopped() {
                                return Ok(());
                            }
                            warn!("Capture unavailable after desktop switch ({e:#}); retrying");
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                };
                *shared.display.lock().unwrap() = capturer.display_rect();
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
        }
        if capturer.is_fallback() && Instant::now() >= next_dxgi_retry {
            next_dxgi_retry = Instant::now() + Duration::from_secs(10);
            if let Some(better) = capture::try_preferred(&args) {
                info!("Switched back to {}", better.name());
                capturer = better;
                *shared.display.lock().unwrap() = capturer.display_rect();
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
        }
        if !shared.flow.wait_for_credit(Duration::from_millis(250)) {
            continue;
        }
        let now = Instant::now();
        if now < next_due {
            std::thread::sleep(next_due - now);
        }
        let started = Instant::now();
        next_due = started + interval;

        let want_keyframe = shared.keyframe_requested.swap(false, Ordering::SeqCst);
        let to_i420 = |frame: capture::Frame<'_>, picture: &mut I420Buffer| {
            let factor = scale_factor(frame.width, frame.height, session.max_width.load(Ordering::SeqCst) as usize);
            crate::color::bgra_to_i420(frame.data, frame.width, frame.height, frame.stride, factor, picture);
        };

        // Grab a new image (waits up to 100 ms for the screen to change).
        let mut have_picture = false;
        let mut switch_to_gdi = false;
        let failure = match capturer.capture(Duration::from_millis(100)) {
            Ok(Some(frame)) => {
                to_i420(frame, &mut picture);
                have_picture = true;
                idle_misses = 0;
                None
            }
            Ok(None) => {
                // DXGI often returns nothing until the desktop changes, so a
                // still screen never sent a first frame and the controller
                // sat on "Opening" after the password was accepted.
                idle_misses += 1;
                if picture.width == 0 && idle_misses >= 5 && capturer.name().contains("DXGI") {
                    switch_to_gdi = true;
                }
                None
            }
            Err(e) => Some(e),
        };
        if switch_to_gdi {
            info!("No frame from {} yet; taking the first picture with GDI", capturer.name());
            session.notify("Waiting for the first picture from this computer.");
            match capture::open_gdi_fallback(args.display) {
                Ok(next) => {
                    capturer = next;
                    idle_misses = 0;
                    info!("Capturing {} with {}", describe(capturer.display_rect()), capturer.name());
                    *shared.display.lock().unwrap() = capturer.display_rect();
                }
                Err(e) => warn!("GDI capture failed: {e:#}"),
            }
        }
        if let Some(e) = failure {
            errors += 1;
            if want_keyframe {
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
            if errors >= 3 {
                warn!("Capture error ({e:#}); restarting capture");
                // Release the old capturer (and its desktop duplication)
                // before opening a new one: an output can only be duplicated once.
                drop(capturer);
                std::thread::sleep(Duration::from_millis(500));
                capturer = capture::open(&args)?;
                errors = 0;
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
            continue;
        }
        // Session changes (RDP connect/disconnect, tscon, fast user switch,
        // monitor unplugged): tell the controller what happened and when it
        // is back, without waiting for a black picture.
        if facts_checked.elapsed() >= Duration::from_secs(2) {
            facts_checked = Instant::now();
            let problem = SessionFacts::now().problem();
            if problem != facts_problem {
                match &problem {
                    Some(p) => {
                        warn!("Session state: {p}");
                        if let Some(state) = SessionFacts::now().state() {
                            session.notify(&pjdesk_proto::sessionnote::text(&crate::hostname(), state));
                        }
                    }
                    None => {
                        info!("Session state is normal again");
                        session.notify(&pjdesk_proto::sessionnote::text(
                            &crate::hostname(),
                            pjdesk_proto::sessionnote::SessionState::Back,
                        ));
                        shared.keyframe_requested.store(true, Ordering::SeqCst);
                    }
                }
                facts_problem = problem;
            }
        }
        if have_picture {
            errors = 0;
            if luma_is_black(&picture.y) {
                let since = *black_since.get_or_insert_with(Instant::now);
                if !black_told && since.elapsed() >= Duration::from_secs(3) {
                    black_told = true;
                    let reason = black_capture_reason(capturer.name(), &SessionFacts::now());
                    warn!("Captured picture is black with {} ({reason})", capturer.name());
                    session.notify(&format!("The remote screen can't be captured: {reason}"));
                    // Try Desktop Duplication again soon instead of in 10 s.
                    next_dxgi_retry = Instant::now();
                }
            } else {
                if black_told {
                    info!("Captured picture has content again");
                }
                black_since = None;
                black_told = false;
            }
        }
        // The display can move or change size (resolution change, monitor layout).
        {
            let rect = capturer.display_rect();
            let mut display = shared.display.lock().unwrap();
            if *display != rect {
                info!("Now capturing {}", describe(rect));
                *display = rect;
            }
        }
        let mut refine_now = false;
        if have_picture {
            refine.changed(Instant::now());
        } else if picture.width > 0 && refine.due(Instant::now()) {
            refine_now = true;
            have_picture = true;
        }
        if !have_picture && (want_keyframe || undelivered) && picture.width > 0 {
            // Nothing changed, but the controller needs a keyframe or never got
            // the latest image: encode the newest picture again.
            have_picture = true;
        }
        if !have_picture {
            continue;
        }

        if want_keyframe || refine_now {
            encoder.request_keyframe();
        }
        let Some((data, keyframe)) = encoder.encode(&picture)? else {
            undelivered = true;
            if want_keyframe {
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
            continue;
        };
        undelivered = false;
        seq = seq.wrapping_add(1);
        let header = VideoHeader {
            seq,
            keyframe,
            width: picture.width as u16,
            height: picture.height as u16,
            encode_us: started.elapsed().as_micros().min(u32::MAX as u128) as u32,
        };
        shared.flow.sent(seq);
        session.note_sent(seq);
        if frames.blocking_send(EncodedFrame { header, data }).is_err() {
            break;
        }
    }
    Ok(())
}

/// True when the luma plane is (almost) all black.
fn luma_is_black(y: &[u8]) -> bool {
    if y.is_empty() {
        return false;
    }
    let step = (y.len() / 4096).max(1);
    let (mut seen, mut bright) = (0usize, 0usize);
    for v in y.iter().step_by(step) {
        seen += 1;
        if *v > 40 {
            bright += 1;
        }
    }
    bright * 1000 < seen
}

/// What this process can see about its Windows session, for telling the
/// controller clearly why the picture or input may not work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionFacts {
    pub session: Option<u32>,
    /// The session the physical console shows (WTSGetActiveConsoleSessionId).
    pub console: Option<u32>,
    /// This session is a Remote Desktop (RDP) session.
    pub remote: bool,
    /// This session has no one connected (RDP closed or disconnected).
    pub disconnected: bool,
    /// The input desktop name (Winlogon = lock screen, sign-in or UAC).
    pub desktop: String,
    pub monitors: i32,
}

impl SessionFacts {
    #[cfg(windows)]
    pub fn now() -> SessionFacts {
        use windows::Win32::System::RemoteDesktop::{
            WTSActive, WTSConnectState, WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW,
            WTS_CONNECTSTATE_CLASS, WTS_CURRENT_SERVER_HANDLE,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_REMOTESESSION};
        let session = crate::platform::session_id();
        let console = unsafe { WTSGetActiveConsoleSessionId() };
        let mut disconnected = false;
        if let Some(id) = session {
            unsafe {
                let mut buf = windows::core::PWSTR::null();
                let mut len = 0u32;
                if WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), id, WTSConnectState, &mut buf, &mut len).is_ok() && !buf.is_null() {
                    let state = *(buf.0 as *const WTS_CONNECTSTATE_CLASS);
                    disconnected = state != WTSActive;
                    WTSFreeMemory(buf.0 as _);
                }
            }
        }
        SessionFacts {
            session,
            console: (console != u32::MAX).then_some(console),
            remote: unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0,
            disconnected,
            desktop: crate::platform::input_desktop_name().unwrap_or_default(),
            monitors: unsafe { GetSystemMetrics(SM_CMONITORS) },
        }
    }

    #[cfg(not(windows))]
    pub fn now() -> SessionFacts {
        SessionFacts { monitors: 1, ..Default::default() }
    }

    /// The short state the controller is told about, matching [`Self::problem`].
    pub fn state(&self) -> Option<pjdesk_proto::sessionnote::SessionState> {
        use pjdesk_proto::sessionnote::SessionState as S;
        if self.session == Some(0) {
            Some(S::NoDesktop)
        } else if self.remote {
            Some(S::RemoteDesktop)
        } else if self.disconnected {
            Some(S::NotOnScreen)
        } else if self.console.is_some_and(|c| Some(c) != self.session) {
            Some(S::OtherSession)
        } else if self.monitors <= 0 {
            Some(S::NoMonitor)
        } else {
            None
        }
    }

    /// A plain reason when this session can't show a normal desktop, else None.
    pub fn problem(&self) -> Option<String> {
        let id = self.session.unwrap_or(0);
        if self.session == Some(0) {
            return Some("PJDesk is running in Windows session 0, which has no desktop. Install the PJDesk service so it starts in the signed-in session.".into());
        }
        if self.remote {
            return Some(format!("this computer is being used through Remote Desktop (session {id}), so its console screen is not shown. Close Remote Desktop, or on that computer run as administrator: tscon {id} /dest:console"));
        }
        if self.disconnected {
            return Some(format!("the Windows session (session {id}) is disconnected, usually after Remote Desktop was closed, so Windows draws nothing. On that computer run as administrator: tscon {id} /dest:console  — or sign in at the computer."));
        }
        if self.console.is_some_and(|c| Some(c) != self.session) {
            return Some("another Windows session is on the screen (fast user switching or a different sign-in). PJDesk follows it when its service is installed.".into());
        }
        if self.monitors <= 0 {
            return Some("no monitor is connected or turned on. Connect a monitor or a display emulator (HDMI dummy plug).".into());
        }
        None
    }

    /// Short state for the viewer when nothing is wrong but the screen is
    /// the lock, sign-in or UAC screen.
    pub fn secure_desktop(&self) -> bool {
        self.desktop.eq_ignore_ascii_case("winlogon")
    }
}

/// Why a captured picture is black, as far as this process can tell.
fn black_capture_reason(backend: &str, facts: &SessionFacts) -> String {
    if let Some(problem) = facts.problem() {
        return problem;
    }
    if facts.secure_desktop() {
        return "the lock, sign-in or administrator (UAC) screen is up and could not be captured. Install the PJDesk service to see and use it.".into();
    }
    if backend.contains("GDI") {
        return "Windows gives only a black picture. No monitor may be connected or turned on, or this computer's Remote Desktop session was disconnected. Connect a monitor or a display emulator (HDMI dummy plug), or sign in at the console.".into();
    }
    "the screen is black. The display may be off or asleep, or a full-screen program is drawing black.".into()
}

fn describe(rect: Rect) -> String {
    format!("{}x{} display at ({}, {})", rect.width, rect.height, rect.left, rect.top)
}

/// Keeps the host awake (no sleep / screen off) while a session runs.
struct KeepAwake;

impl KeepAwake {
    fn new() -> Self {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::System::Power::{
                SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
            };
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
        }
        KeepAwake
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_factors() {
        assert_eq!(scale_factor(1920, 1080, 2560), 1);
        assert_eq!(scale_factor(2560, 1440, 2560), 1);
        assert_eq!(scale_factor(3840, 2160, 2560), 2);
        assert_eq!(scale_factor(3840, 2160, 3840), 1);
        assert_eq!(scale_factor(5120, 1440, 5120), 2); // 5120 wide exceeds OpenH264's limit
    }

    #[test]
    fn flow_control_limits_frames_in_flight() {
        let flow = Flow::new();
        for seq in 1..=MAX_IN_FLIGHT {
            assert!(flow.wait_for_credit(Duration::from_millis(1)));
            flow.sent(seq);
        }
        assert!(!flow.wait_for_credit(Duration::from_millis(10)));
        flow.ack(1);
        assert!(flow.wait_for_credit(Duration::from_millis(1)));
        flow.ack(99); // ack beyond what was sent is ignored
        flow.sent(4);
        assert!(!flow.wait_for_credit(Duration::from_millis(10)));
        flow.stop();
        assert!(!flow.wait_for_credit(Duration::from_millis(10)));
    }
}

#[cfg(test)]
mod black_capture_tests {
    use super::*;

    #[test]
    fn a_black_capture_is_explained() {
        assert!(luma_is_black(&vec![16u8; 1920 * 1080]));
        let mut y = vec![16u8; 1920 * 1080];
        for v in y.iter_mut().step_by(5) {
            *v = 200;
        }
        assert!(!luma_is_black(&y));
        let normal = SessionFacts { session: Some(1), console: Some(1), monitors: 1, ..Default::default() };
        assert!(black_capture_reason("GDI BitBlt", &normal).contains("monitor"));
        let zero = SessionFacts { session: Some(0), ..normal.clone() };
        assert!(black_capture_reason("DXGI", &zero).contains("session 0"));
    }

    #[test]
    fn session_facts_name_rdp_disconnect_switch_lock_and_no_monitor() {
        let normal = SessionFacts { session: Some(2), console: Some(2), monitors: 1, desktop: "Default".into(), ..Default::default() };
        assert_eq!(normal.problem(), None);
        let rdp = SessionFacts { remote: true, ..normal.clone() };
        assert!(rdp.problem().unwrap().contains("tscon 2 /dest:console"));
        let gone = SessionFacts { disconnected: true, ..normal.clone() };
        assert!(gone.problem().unwrap().contains("disconnected"));
        assert!(gone.problem().unwrap().contains("tscon 2"));
        let switched = SessionFacts { console: Some(5), ..normal.clone() };
        assert!(switched.problem().unwrap().contains("another Windows session"));
        let dark = SessionFacts { monitors: 0, ..normal.clone() };
        assert!(dark.problem().unwrap().contains("monitor"));
        let locked = SessionFacts { desktop: "Winlogon".into(), ..normal };
        assert_eq!(locked.problem(), None);
        assert!(black_capture_reason("DXGI", &locked).contains("lock"));
    }
}
