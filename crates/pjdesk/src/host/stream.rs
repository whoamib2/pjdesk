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
use pjdesk_proto::framing::{read_msg, write_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::peer::{write_video_frame, ControlMsg, HostMsg, VideoHeader};
use quinn::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::capture::{self, Rect};
use super::encoder::VideoEncoder;
use super::input::InputInjector;
use crate::color::{scaled_size, I420Buffer};
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
) -> Result<()> {
    let shared = Arc::new(Shared {
        flow: Flow::new(),
        keyframe_requested: AtomicBool::new(false),
        display: Mutex::new(Rect::default()),
    });

    let (frame_tx, mut frame_rx) = mpsc::channel::<EncodedFrame>(MAX_IN_FLIGHT as usize + 1);
    let capture_thread = {
        let shared = shared.clone();
        let args = args.clone();
        std::thread::Builder::new().name("capture".into()).spawn(move || {
            if let Err(e) = capture_loop(&args, &shared, &frame_tx) {
                warn!("Screen capture stopped: {e:#}");
            }
        })?
    };

    let mut video = connection.open_uni().await?;
    // Input and acks on the control stream go before video bytes.
    let _ = video.set_priority(-1);
    let mut video_task = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            write_video_frame(&mut video, &frame.header, &frame.data).await?;
        }
        let _ = video.finish();
        Ok::<_, anyhow::Error>(())
    });

    let mut injector = InputInjector::new(args.view_only);
    let result = loop {
        tokio::select! {
            msg = read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME) => match msg {
                Ok(ControlMsg::Input(event)) => {
                    let display = *shared.display.lock().unwrap();
                    injector.handle(event, display);
                }
                Ok(ControlMsg::FrameAck { seq }) => shared.flow.ack(seq),
                Ok(ControlMsg::RequestKeyframe) => shared.keyframe_requested.store(true, Ordering::SeqCst),
                Ok(ControlMsg::Special(cmd)) => {
                    if !args.view_only {
                        match cmd {
                            pjdesk_proto::peer::SpecialCommand::CtrlAltDel => crate::platform::send_ctrl_alt_del(),
                        }
                    }
                }
                Ok(ControlMsg::Ping { t_us }) => {
                    if let Err(e) = write_msg(&mut control_send, &HostMsg::Pong { t_us }).await {
                        break Err(e);
                    }
                }
                // The controller closed the stream or the connection.
                Err(_) => break Ok(()),
            },
            done = &mut video_task => break match done {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e).context("sending video"),
                Err(e) => Err(e.into()),
            },
            _ = connection.closed() => break Ok(()),
        }
    };

    shared.flow.stop();
    injector.release_all();
    video_task.abort();
    let _ = tokio::task::spawn_blocking(move || capture_thread.join()).await;
    result
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
fn capture_loop(args: &HostArgs, shared: &Shared, frames: &mpsc::Sender<EncodedFrame>) -> Result<()> {
    let _awake = KeepAwake::new();
    // Service helper: attach this capture thread to the input desktop so it can
    // see UAC prompts, the lock screen and the sign-in screen.
    let mut desktop = String::new();
    if args.follow_desktop {
        crate::platform::attach_thread_to_input_desktop();
        desktop = crate::platform::input_desktop_name().unwrap_or_default();
    }
    let mut capturer = capture::open(args)?;
    info!("Capturing {} with {}", describe(capturer.display_rect()), capturer.name());
    *shared.display.lock().unwrap() = capturer.display_rect();

    let mut encoder = VideoEncoder::new(args.bitrate, args.fps)?;
    let interval = Duration::from_secs_f64(1.0 / args.fps.clamp(1, 120) as f64);
    let mut next_due = Instant::now();
    let mut seq = 0u32;
    let mut picture = I420Buffer::default();
    let mut errors = 0u32;
    // The newest image in `picture` hasn't reached the controller yet
    // (the encoder dropped it to hold the bitrate).
    let mut undelivered = false;
    // When GDI is only a fallback, keep trying to get back to DXGI
    // (it may have been unavailable at start, e.g. during a UAC prompt).
    let mut next_dxgi_retry = Instant::now() + Duration::from_secs(10);

    while !shared.flow.stopped() {
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
                    match capture::open(args) {
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
            if let Some(better) = capture::try_preferred(args) {
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
            let factor = scale_factor(frame.width, frame.height, args.max_width as usize);
            crate::color::bgra_to_i420(frame.data, frame.width, frame.height, frame.stride, factor, picture);
        };

        // Grab a new image (waits up to 100 ms for the screen to change).
        let mut have_picture = false;
        let failure = match capturer.capture(Duration::from_millis(100)) {
            Ok(Some(frame)) => {
                to_i420(frame, &mut picture);
                have_picture = true;
                None
            }
            Ok(None) => None,
            Err(e) => Some(e),
        };
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
                capturer = capture::open(args)?;
                errors = 0;
                shared.keyframe_requested.store(true, Ordering::SeqCst);
            }
            continue;
        }
        if have_picture {
            errors = 0;
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
        if !have_picture && (want_keyframe || undelivered) && picture.width > 0 {
            // Nothing changed, but the controller needs a keyframe or never got
            // the latest image: encode the newest picture again.
            have_picture = true;
        }
        if !have_picture {
            continue;
        }

        if want_keyframe {
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
        if frames.blocking_send(EncodedFrame { header, data }).is_err() {
            break;
        }
    }
    Ok(())
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
