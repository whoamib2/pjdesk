//! Messages on the QUIC connection between controller and host.
//!
//! Streams:
//! * **Control** (bidirectional, opened by the controller): the SPAKE2
//!   handshake ([`AuthMsg`]), then [`ControlMsg`] from controller to host
//!   (input, frame acks, keyframe requests) and [`HostMsg`] back.
//! * **Video** (unidirectional, opened by the host after authentication):
//!   a sequence of H.264 access units, each `[u16 header len][VideoHeader]
//!   [u32 data len][Annex-B bitstream]`.
//!
//! Latency control: the host keeps at most a few frames "in flight" (sent but
//! not yet acknowledged by the controller's decoder) and simply captures less
//! often when the network or the decoder is slow, so frames never queue up.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest encoded frame accepted (a 4K keyframe is far below this).
pub const MAX_VIDEO_FRAME: usize = 16 * 1024 * 1024;
/// Largest video header accepted. A header is a few dozen bytes; anything
/// larger is a peer trying to make the host or controller allocate.
pub const MAX_VIDEO_HEADER: usize = 4096;

/// Password-authenticated key exchange (see [`crate::auth`]).
///
/// The host may accept more than one password at once (a persistent permanent
/// password and/or a rotating one-time password). The controller therefore
/// sends one SPAKE2 message per "slot" with the single password it is trying,
/// and the host replies with one message + confirmation per password it offers;
/// the controller then names the slot whose confirmation verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMsg {
    /// Controller → host: one SPAKE2 message A per candidate slot (same password).
    Start { proto: u16, spakes: Vec<Vec<u8>>, controller_name: String },
    /// Host → controller: a SPAKE2 message B + key confirmation for each
    /// password the host accepts (index-aligned with the controller's slots).
    Challenge { spakes: Vec<Vec<u8>>, confirms: Vec<[u8; 32]> },
    /// Controller → host: which slot matched (`0xFF` = none) and its confirmation.
    Confirm { slot: u8, confirm: [u8; 32] },
    /// Host → controller: final verdict.
    Result { ok: bool, message: String, host_name: String },
}

/// Controller → host messages after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMsg {
    Input(InputEvent),
    /// The controller decoded frame `seq`; the host may send another.
    FrameAck {
        seq: u32,
    },
    /// The decoder lost sync; please send an IDR frame.
    RequestKeyframe,
    /// A special command the host performs itself (e.g. Ctrl+Alt+Del).
    Special(SpecialCommand),
    Ping {
        t_us: u64,
    },
}

/// Commands the controller asks the host to carry out directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialCommand {
    /// Secure attention sequence (Ctrl+Alt+Del). Works when the host runs as
    /// the background service; a normal app can't inject it.
    CtrlAltDel,
}

/// Host → controller messages after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    Pong {
        t_us: u64,
    },
    /// Human-readable status for the controller's title bar / console.
    Notice(String),
}

/// Input from the controller, replayed on the host with `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Absolute pointer position, normalized to the shared display:
    /// 0 = left/top edge, 65535 = right/bottom edge.
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Wheel movement in Windows units (120 = one notch).
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// A physical key, as a PC/AT "set 1" scan code. Extended keys carry the
    /// 0xE0 prefix in the high byte (e.g. Left arrow = 0xE04B); Pause is 0xE11D.
    /// Numpad keys typed with the controller's NumLock on carry
    /// [`KEY_NUMPAD_DIGIT`], so they produce digits whatever the host's NumLock.
    Key {
        scan: u16,
        down: bool,
    },
    /// Release every key and button the controller may still be holding
    /// (sent when the viewer window loses focus).
    ReleaseAll,
}

/// Flag on [`InputEvent::Key`] scan codes: a numpad key meant as a digit.
pub const KEY_NUMPAD_DIGIT: u16 = 0x8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Metadata for one encoded video frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoHeader {
    pub seq: u32,
    pub keyframe: bool,
    /// Encoded picture size (may be smaller than the host screen if downscaled).
    pub width: u16,
    pub height: u16,
    /// Time spent converting + encoding on the host, in microseconds.
    pub encode_us: u32,
}

/// Writes one encoded frame to the video stream.
pub async fn write_video_frame<W>(w: &mut W, header: &VideoHeader, data: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let head = postcard::to_stdvec(header)?;
    let mut prefix = Vec::with_capacity(2 + head.len() + 4);
    prefix.extend_from_slice(&(head.len() as u16).to_be_bytes());
    prefix.extend_from_slice(&head);
    prefix.extend_from_slice(&(data.len() as u32).to_be_bytes());
    w.write_all(&prefix).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

/// Reads one encoded frame into `data` (reusing its allocation).
pub async fn read_video_frame<R>(r: &mut R, data: &mut Vec<u8>) -> Result<VideoHeader>
where
    R: AsyncRead + Unpin,
{
    let mut len2 = [0u8; 2];
    r.read_exact(&mut len2).await?;
    let head_len = u16::from_be_bytes(len2) as usize;
    if head_len > MAX_VIDEO_HEADER {
        bail!("video header too large ({head_len} bytes)");
    }
    let mut head = vec![0u8; head_len];
    r.read_exact(&mut head).await?;
    let header: VideoHeader = postcard::from_bytes(&head)?;
    let mut len4 = [0u8; 4];
    r.read_exact(&mut len4).await?;
    let len = u32::from_be_bytes(len4) as usize;
    if len > MAX_VIDEO_FRAME {
        bail!("video frame too large ({len} bytes)");
    }
    data.resize(len, 0);
    r.read_exact(data).await?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn video_frame_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let header = VideoHeader { seq: 9, keyframe: true, width: 1920, height: 1080, encode_us: 1234 };
        let payload: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        write_video_frame(&mut a, &header, &payload).await.unwrap();
        let mut got = Vec::new();
        let h = read_video_frame(&mut b, &mut got).await.unwrap();
        assert_eq!(h.seq, 9);
        assert!(h.keyframe);
        assert_eq!((h.width, h.height), (1920, 1080));
        assert_eq!(got, payload);
    }

    #[tokio::test]
    async fn rejects_oversized_video_header() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&((MAX_VIDEO_HEADER as u16) + 1).to_be_bytes()).await.unwrap();
        let mut got = Vec::new();
        assert!(read_video_frame(&mut b, &mut got).await.is_err());
    }
}
