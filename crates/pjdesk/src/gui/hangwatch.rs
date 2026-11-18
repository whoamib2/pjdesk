//! Watch the UI thread and keep a parked window from being asked to present.
//!
//! A minimized PJDesk window can still be woken for a frame. On Windows,
//! presenting that frame can stop the message loop, and the window stays
//! "(Not Responding)". Background threads must not request a repaint while
//! the window is parked. If a frame still runs for more than two seconds,
//! this module logs the last UI operation so the hang can be found.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;

const HANG_AFTER: Duration = Duration::from_secs(2);
const HUNG_CHECK_EVERY: Duration = Duration::from_secs(2);

static PARKED: AtomicBool = AtomicBool::new(false);
static IN_FRAME: AtomicBool = AtomicBool::new(false);
static IN_UI: AtomicBool = AtomicBool::new(false);
static STARTED: OnceLock<()> = OnceLock::new();
static LAST_OP: Mutex<&'static str> = Mutex::new("idle");
static FRAME_START: Mutex<Option<Instant>> = Mutex::new(None);
static LAST_HUNG_CHECK: Mutex<Option<Instant>> = Mutex::new(None);
static HANG_LOGGED: AtomicBool = AtomicBool::new(false);

/// The window is minimized or hidden, so a present would be unsafe.
pub fn set_parked(parked: bool) {
    PARKED.store(parked, Ordering::SeqCst);
}

pub fn is_parked() -> bool {
    PARKED.load(Ordering::SeqCst)
}

/// Wake the UI only when the window is visible.
pub fn request_repaint_if_visible(ctx: &egui::Context) {
    if !is_parked() {
        ctx.request_repaint();
    }
}

pub fn mark(op: &'static str) {
    if let Ok(mut slot) = LAST_OP.lock() {
        *slot = op;
    }
}

pub fn last_op() -> &'static str {
    LAST_OP.lock().map(|slot| *slot).unwrap_or("unknown")
}

/// Start of a UI frame on the window thread.
pub fn begin_frame() {
    start_watcher();
    HANG_LOGGED.store(false, Ordering::SeqCst);
    if let Ok(mut check) = LAST_HUNG_CHECK.lock() {
        *check = None;
    }
    mark("frame start");
    if let Ok(mut start) = FRAME_START.lock() {
        *start = Some(Instant::now());
    }
    IN_UI.store(true, Ordering::SeqCst);
    IN_FRAME.store(true, Ordering::SeqCst);
}

/// End of UI work. If the window is parked, the present must not be treated
/// as still running: no further frame may be scheduled. If it is visible,
/// leave the frame open so a hung present is still logged. An idle window
/// that keeps pumping messages is not a hang.
pub fn end_frame(parked: bool) {
    set_parked(parked);
    IN_UI.store(false, Ordering::SeqCst);
    if parked {
        mark("frame end, parked");
        IN_FRAME.store(false, Ordering::SeqCst);
    } else {
        mark("present");
    }
}

fn start_watcher() {
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("ui-hangwatch".into()).spawn(watch_loop);
    });
}

fn watch_loop() {
    loop {
        std::thread::sleep(Duration::from_millis(400));
        if !IN_FRAME.load(Ordering::SeqCst) {
            continue;
        }
        let Some(start) = FRAME_START.lock().ok().and_then(|slot| *slot) else { continue };
        let elapsed = start.elapsed();
        if elapsed < HANG_AFTER {
            continue;
        }
        if IN_UI.load(Ordering::SeqCst) {
            log_hang(elapsed);
            continue;
        }
        if !due_hung_check() {
            continue;
        }
        if crate::winshow::window_is_hung("PJDesk") {
            log_hang(elapsed);
        }
    }
}

fn due_hung_check() -> bool {
    let Ok(mut slot) = LAST_HUNG_CHECK.lock() else {
        return false;
    };
    if slot.is_some_and(|at| at.elapsed() < HUNG_CHECK_EVERY) {
        return false;
    }
    *slot = Some(Instant::now());
    true
}

fn log_hang(elapsed: Duration) {
    if HANG_LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    let op = last_op();
    tracing::warn!(
        "The PJDesk window thread has been in one frame for {:.1} s. Last operation: {op}. The window may show Not Responding.",
        elapsed.as_secs_f32()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_window_is_not_treated_as_visible() {
        set_parked(false);
        assert!(!is_parked());
        set_parked(true);
        assert!(is_parked());
        set_parked(false);
        assert!(!is_parked());
    }

    #[test]
    fn mark_keeps_the_last_operation() {
        mark("poll_probe");
        assert_eq!(last_op(), "poll_probe");
        mark("idle");
    }

    #[test]
    fn ending_a_parked_frame_clears_the_in_frame_flag() {
        IN_FRAME.store(true, Ordering::SeqCst);
        IN_UI.store(true, Ordering::SeqCst);
        end_frame(true);
        assert!(!IN_FRAME.load(Ordering::SeqCst));
        assert!(!IN_UI.load(Ordering::SeqCst));
        assert!(is_parked());
        set_parked(false);
    }

    #[test]
    fn ending_a_visible_frame_keeps_watch_on_the_present() {
        set_parked(false);
        IN_FRAME.store(true, Ordering::SeqCst);
        IN_UI.store(true, Ordering::SeqCst);
        end_frame(false);
        assert!(IN_FRAME.load(Ordering::SeqCst));
        assert!(!IN_UI.load(Ordering::SeqCst));
        assert_eq!(last_op(), "present");
        IN_FRAME.store(false, Ordering::SeqCst);
    }
}
