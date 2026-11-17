//! The controller's control-stream writer, shared by the Windows and Android
//! viewers.
//!
//! Pointer moves are absolute, so only the newest one already queued matters.
//! Moves are not delayed to wait for more; they go out at once at the input
//! rate, with only a same-batch coalesce to the latest position. Everything
//! else (buttons, keys, wheel, acks) is never merged or dropped, keeps its
//! order, and goes out right after the pointer position it belongs to.

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::peer::{ControlMsg, InputEvent};

fn is_move(msg: &ControlMsg) -> bool {
    matches!(msg, ControlMsg::Input(InputEvent::MouseMove { .. }) | ControlMsg::Pointer { .. })
}

/// Drops every move that a later move in the same run replaces. A run ends at
/// any message that is not a move, so a click always lands where the pointer
/// was when it was pressed. A `Pointer` on another display is kept.
pub fn coalesce_moves(batch: &mut Vec<ControlMsg>) {
    let mut out: Vec<ControlMsg> = Vec::with_capacity(batch.len());
    for msg in batch.drain(..) {
        if is_move(&msg) {
            if let Some(last) = out.last() {
                let replaces = match (last, &msg) {
                    (ControlMsg::Input(InputEvent::MouseMove { .. }), ControlMsg::Input(InputEvent::MouseMove { .. })) => true,
                    (ControlMsg::Pointer { display: a, .. }, ControlMsg::Pointer { display: b, .. }) => a == b,
                    _ => false,
                };
                if replaces {
                    *out.last_mut().unwrap() = msg;
                    continue;
                }
            }
        }
        out.push(msg);
    }
    *batch = out;
}

/// Writes control messages until the channel closes or the stream fails.
pub async fn pump_control<W: AsyncWrite + Unpin>(mut send: W, mut rx: UnboundedReceiver<ControlMsg>) {
    let mut batch: Vec<ControlMsg> = Vec::new();
    let mut buf = Vec::new();
    while let Some(first) = rx.recv().await {
        batch.push(first);
        while let Ok(msg) = rx.try_recv() {
            batch.push(msg);
        }
        coalesce_moves(&mut batch);
        buf.clear();
        for msg in batch.drain(..) {
            if let Ok(body) = postcard::to_stdvec(&msg) {
                buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
                buf.extend_from_slice(&body);
            }
        }
        if send.write_all(&buf).await.is_err() || send.flush().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::{read_msg, MAX_CONTROL_FRAME};
    use crate::peer::MouseButton;

    fn mv(x: u16) -> ControlMsg {
        ControlMsg::Input(InputEvent::MouseMove { x, y: x })
    }
    fn enc(msgs: &[ControlMsg]) -> Vec<Vec<u8>> {
        msgs.iter().map(|m| postcard::to_stdvec(m).unwrap()).collect()
    }
    fn button(down: bool) -> ControlMsg {
        ControlMsg::Input(InputEvent::MouseButton { button: MouseButton::Left, down })
    }

    #[test]
    fn moves_merge_but_clicks_keep_their_place() {
        let mut batch = vec![mv(1), mv(2), mv(3), button(true), mv(4), mv(5), button(false), mv(6)];
        coalesce_moves(&mut batch);
        assert_eq!(enc(&batch), enc(&[mv(3), button(true), mv(5), button(false), mv(6)]));
    }

    #[test]
    fn pointers_on_different_displays_are_kept() {
        let p = |display, x| ControlMsg::Pointer { display, x, y: 0 };
        let mut batch = vec![p(0, 1), p(0, 2), p(1, 3), p(1, 4)];
        coalesce_moves(&mut batch);
        assert_eq!(enc(&batch), enc(&[p(0, 2), p(1, 4)]));
    }

    #[tokio::test]
    async fn a_queued_burst_of_moves_becomes_one_and_a_click_is_never_lost() {
        let (a, mut b) = tokio::io::duplex(64 * 1024);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for x in 1..=9 {
            tx.send(mv(x)).unwrap();
        }
        tx.send(button(true)).unwrap();
        tx.send(button(false)).unwrap();
        let pump = tokio::spawn(pump_control(a, rx));
        drop(tx);
        let mut rest = Vec::new();
        while let Ok(msg) = read_msg::<_, ControlMsg>(&mut b, MAX_CONTROL_FRAME).await {
            rest.push(msg);
        }
        assert_eq!(enc(&rest), enc(&[mv(9), button(true), button(false)]));
        pump.await.unwrap();
    }
}
