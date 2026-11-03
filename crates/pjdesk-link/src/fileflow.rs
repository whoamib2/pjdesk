//! Flow control for file data, so input, control messages and the picture
//! never wait behind a file.
//!
//! QUIC stream priority only orders what is already inside the connection.
//! A file written as fast as QUIC accepts it fills the connection's whole
//! send window (16 MB). Writes on the control stream then block until those
//! bytes are acknowledged, which over the TCP relay (2 MB/s) took seconds:
//! the mouse and keyboard froze until the transfer ended.
//!
//! The sender now keeps at most a window of file bytes that the receiver has
//! not confirmed with `FileMsg::Progress` (every receiver since 0.6.0 sends one
//! every 256 KB). The window follows the delivered rate so that file data
//! queues for about [`QUEUE_TARGET`] at most. A receiver that never confirms
//! (an older phone, for example) gets delay-based pacing instead.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// File data may queue this long on the path before the sender waits.
pub const QUEUE_TARGET: Duration = Duration::from_millis(120);
pub const MIN_WINDOW: u64 = 512 * 1024;
pub const MAX_WINDOW: u64 = 8 * 1024 * 1024;
pub const START_WINDOW: u64 = 1024 * 1024;
/// File chunks are written this small, so other streams interleave finely.
pub const CHUNK: usize = 16 * 1024;
/// How often a receiver confirms what it has written.
pub const ACK_EVERY: u64 = 256 * 1024;
/// Without any confirmation after this much, pace on round-trip time instead.
const NO_ACK_AFTER: u64 = 2 * 1024 * 1024;

/// Receiver confirmations for one transfer.
#[derive(Default)]
pub struct Flow {
    acked: AtomicU64,
}

type Key = (usize, u64);
fn flows() -> &'static Mutex<HashMap<Key, Arc<Flow>>> {
    static FLOWS: OnceLock<Mutex<HashMap<Key, Arc<Flow>>>> = OnceLock::new();
    FLOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The receiver reported `bytes` written for transfer `id` on `conn`.
pub fn ack(conn: &quinn::Connection, id: u64, bytes: u64) {
    if let Some(flow) = flows().lock().unwrap().get(&(conn.stable_id(), id)) {
        flow.acked.fetch_max(bytes, Ordering::SeqCst);
    }
}

/// The pure window arithmetic, separate so it can be tested without a network.
#[derive(Debug, Clone)]
pub struct Window {
    pub window: u64,
    pub base_rtt: Duration,
    mark_at: Option<Instant>,
    mark_acked: u64,
}

impl Default for Window {
    fn default() -> Self {
        Self { window: START_WINDOW, base_rtt: Duration::from_secs(10), mark_at: None, mark_acked: 0 }
    }
}

impl Window {
    /// Updates the window from a confirmation and the current round trip.
    pub fn on_ack(&mut self, acked: u64, rtt: Duration, now: Instant) {
        self.base_rtt = self.base_rtt.min(rtt);
        let Some(at) = self.mark_at else {
            self.mark_at = Some(now);
            self.mark_acked = acked;
            return;
        };
        let dt = now.saturating_duration_since(at);
        if dt < Duration::from_millis(200) || acked <= self.mark_acked {
            return;
        }
        let rate = (acked - self.mark_acked) as f64 / dt.as_secs_f64();
        // Room for what is in flight plus QUEUE_TARGET of queue; 1.25 lets the
        // window grow while the queue stays short and shrink once it grows.
        let want = rate * (self.base_rtt + QUEUE_TARGET).as_secs_f64() * 1.25;
        self.window = (want as u64).clamp(MIN_WINDOW, MAX_WINDOW);
        self.mark_at = Some(now);
        self.mark_acked = acked;
    }

    /// True when another chunk may go out.
    pub fn may_send(&self, sent: u64, acked: u64) -> bool {
        sent.saturating_sub(acked) < self.window
    }
}

/// Fallback without confirmations: wait while the round trip is inflated.
pub fn delay_ok(rtt: Duration, base: Duration) -> bool {
    rtt <= base + QUEUE_TARGET + Duration::from_millis(80)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// No confirmation yet: up to one window goes out, then wait for one.
    Probing,
    /// The receiver confirms: window flow control.
    Acked,
    /// The receiver never confirms: pace on round-trip time.
    DelayOnly,
}

/// Paces one outgoing file. Drop it when the transfer ends.
pub struct Pacer {
    conn: quinn::Connection,
    key: Key,
    flow: Arc<Flow>,
    start: u64,
    mode: Mode,
    w: Window,
}

impl Pacer {
    /// `resume` is where the transfer starts; the receiver already has that much.
    pub fn new(conn: &quinn::Connection, id: u64, resume: u64) -> Self {
        let key = (conn.stable_id(), id);
        let flow = Arc::new(Flow::default());
        flow.acked.store(resume, Ordering::SeqCst);
        flows().lock().unwrap().insert(key, flow.clone());
        let mut w = Window::default();
        w.base_rtt = conn.rtt();
        Self { conn: conn.clone(), key, flow, start: resume, mode: Mode::Probing, w }
    }

    /// Waits until another chunk may follow `sent` bytes (counted from the
    /// file's start). Sleeps in 5 ms steps; never spins.
    pub async fn wait(&mut self, sent: u64) {
        let mut slept = Duration::ZERO;
        loop {
            if self.conn.close_reason().is_some() {
                return;
            }
            let rtt = self.conn.rtt();
            let acked = self.flow.acked.load(Ordering::SeqCst);
            if self.mode == Mode::Probing && acked > self.start {
                self.mode = Mode::Acked;
            }
            match self.mode {
                Mode::Acked => {
                    self.w.on_ack(acked, rtt, Instant::now());
                    if self.w.may_send(sent, acked) {
                        return;
                    }
                }
                Mode::Probing => {
                    self.w.base_rtt = self.w.base_rtt.min(rtt);
                    if sent.saturating_sub(self.start) < NO_ACK_AFTER.min(self.w.window) {
                        return;
                    }
                    if slept >= Duration::from_secs(3) {
                        self.mode = Mode::DelayOnly;
                    }
                }
                Mode::DelayOnly => {
                    self.w.base_rtt = self.w.base_rtt.min(rtt);
                    if delay_ok(rtt, self.w.base_rtt) || slept >= Duration::from_millis(500) {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            slept += Duration::from_millis(5);
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        flows().lock().unwrap().remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_bounds_unconfirmed_file_bytes() {
        let w = Window::default();
        assert!(w.may_send(START_WINDOW - 1, 0));
        assert!(!w.may_send(START_WINDOW, 0));
        assert!(w.may_send(START_WINDOW + 10, 20));
    }

    #[test]
    fn a_slow_relay_gets_a_small_window_so_input_waits_at_most_a_fraction_of_a_second() {
        // 2 MB/s over the TCP relay with a 40 ms round trip.
        let mut w = Window::default();
        let t0 = Instant::now();
        w.on_ack(0, Duration::from_millis(40), t0);
        w.on_ack(2 * 1024 * 1024, Duration::from_millis(40), t0 + Duration::from_secs(1));
        let queue_secs = w.window as f64 / (2.0 * 1024.0 * 1024.0);
        assert!(queue_secs < 0.5, "window {} would queue {queue_secs:.2} s", w.window);
        assert!(w.window >= MIN_WINDOW);
    }

    #[test]
    fn a_fast_link_opens_the_window_up_to_the_cap() {
        let mut w = Window::default();
        let t0 = Instant::now();
        w.on_ack(0, Duration::from_millis(30), t0);
        w.on_ack(100 * 1024 * 1024, Duration::from_millis(30), t0 + Duration::from_secs(1));
        assert_eq!(w.window, MAX_WINDOW);
    }

    #[test]
    fn an_inflated_round_trip_pauses_pacing_without_confirmations() {
        assert!(delay_ok(Duration::from_millis(60), Duration::from_millis(40)));
        assert!(!delay_ok(Duration::from_millis(900), Duration::from_millis(40)));
    }
}
