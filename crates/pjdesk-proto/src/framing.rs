//! Length-prefixed message framing used on every stream.
//!
//! Each message is a 4-byte big-endian length followed by a
//! [postcard](https://docs.rs/postcard) encoded value. Video frames use a
//! slightly different layout (see [`crate::peer`]) so the bitstream is not
//! copied through serde.

use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest signaling message accepted. Account replies carry a sealed
/// password vault, which is larger than a session setup message.
pub const MAX_SIGNAL_FRAME: usize = 256 * 1024;
/// Largest control/auth message accepted on the peer connection.
pub const MAX_CONTROL_FRAME: usize = 256 * 1024;

/// Serializes `msg` and writes it as one frame, then flushes.
pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = postcard::to_stdvec(msg)?;
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// Reads one frame and deserializes it. Fails if the frame exceeds `max_len`.
pub async fn read_msg<R, T>(r: &mut R, max_len: usize) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_len {
        bail!("message too large ({len} bytes, limit {max_len})");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(postcard::from_bytes(&body)?)
}

/// Reads frames like [`read_msg`], but is safe to use as a `tokio::select!`
/// branch. `read_msg` uses `read_exact`, which loses whatever part of a frame
/// it had already read when another branch wins; the stream then reads from
/// the middle of a message, so input is dropped or misread and the session
/// usually ends with a decode error. This keeps the partial frame between
/// calls, and every await point is a single cancel-safe `read`.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
    scratch: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self { buf: Vec::new(), scratch: vec![0u8; 16 * 1024] }
    }

    /// The next whole frame, decoded. Fails if it exceeds `max_len`, does not
    /// decode, or the stream ends.
    pub async fn read_msg<R, T>(&mut self, r: &mut R, max_len: usize) -> Result<T>
    where
        R: AsyncRead + Unpin,
        T: DeserializeOwned,
    {
        if self.scratch.is_empty() {
            self.scratch = vec![0u8; 16 * 1024];
        }
        loop {
            if self.buf.len() >= 4 {
                let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
                if len > max_len {
                    bail!("message too large ({len} bytes, limit {max_len})");
                }
                if self.buf.len() >= 4 + len {
                    let msg = postcard::from_bytes(&self.buf[4..4 + len]);
                    self.buf.drain(..4 + len);
                    return Ok(msg?);
                }
            }
            let n = r.read(&mut self.scratch).await?;
            if n == 0 {
                bail!("the stream ended");
            }
            self.buf.extend_from_slice(&self.scratch[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{ClientMsg, ServerMsg};

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let msg = ClientMsg::Connect { proto: 1, target_id: "123456789".into(), controller_name: "laptop".into() };
        write_msg(&mut a, &msg).await.unwrap();
        write_msg(&mut a, &ServerMsg::Pong).await.unwrap();
        let got: ClientMsg = read_msg(&mut b, MAX_SIGNAL_FRAME).await.unwrap();
        assert!(matches!(got, ClientMsg::Connect { ref target_id, .. } if target_id == "123456789"));
        let got: ServerMsg = read_msg(&mut b, MAX_SIGNAL_FRAME).await.unwrap();
        assert!(matches!(got, ServerMsg::Pong));
    }

    #[tokio::test]
    async fn rejects_oversized_frames() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(10_000_000u32).to_be_bytes()).await.unwrap();
        let r: Result<ServerMsg> = read_msg(&mut b, MAX_SIGNAL_FRAME).await;
        assert!(r.is_err());
    }
    #[tokio::test]
    async fn frame_reader_survives_a_cancelled_read() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let msg = ClientMsg::Connect { proto: 1, target_id: "123456789".into(), controller_name: "laptop".into() };
        let body = postcard::to_stdvec(&msg).unwrap();
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        let mut reader = FrameReader::new();
        // Half the frame arrives, then the read loses a select! race.
        a.write_all(&frame[..7]).await.unwrap();
        let raced = tokio::time::timeout(
            std::time::Duration::from_millis(30),
            reader.read_msg::<_, ClientMsg>(&mut b, MAX_SIGNAL_FRAME),
        )
        .await;
        assert!(raced.is_err(), "nothing whole has arrived yet");
        a.write_all(&frame[7..]).await.unwrap();
        write_msg(&mut a, &ServerMsg::Pong).await.unwrap();
        let got: ClientMsg = reader.read_msg(&mut b, MAX_SIGNAL_FRAME).await.unwrap();
        assert!(matches!(got, ClientMsg::Connect { .. }));
        let got: ServerMsg = reader.read_msg(&mut b, MAX_SIGNAL_FRAME).await.unwrap();
        assert!(matches!(got, ServerMsg::Pong));
    }

    #[tokio::test]
    async fn frame_reader_rejects_oversized_frames() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(10_000_000u32).to_be_bytes()).await.unwrap();
        let r: Result<ServerMsg> = FrameReader::new().read_msg(&mut b, MAX_SIGNAL_FRAME).await;
        assert!(r.is_err());
    }
}
