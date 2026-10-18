//! System-audio capture (host) and playback (controller).
//!
//! The host records the default render device in loopback, resamples to
//! 48 kHz stereo s16, and the controller plays that. There is no microphone.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
#[cfg_attr(not(windows), allow(unused_imports))]
use pjdesk_proto::quality::{resample_s16_interleaved, AUDIO_CHANNELS, AUDIO_FRAME_SAMPLES, AUDIO_RATE};
use tokio::sync::mpsc;

/// Captures loopback audio until `stop` is set, sending 20 ms frames of
/// 48 kHz stereo little-endian s16.
pub fn capture_loop(tx: mpsc::Sender<Vec<u8>>, stop: Arc<AtomicBool>) -> Result<()> {
    #[cfg(windows)]
    {
        capture_wasapi(tx, stop)
    }
    #[cfg(not(windows))]
    {
        let _ = (tx, stop);
        bail!("audio capture is only available on Windows");
    }
}

/// Opens an audio side stream on `conn` and writes loopback frames until
/// `stop` is set. Called from a blocking thread.
pub fn loopback_to(conn: quinn::Connection, stop: Arc<AtomicBool>) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(8);
    let stop_cap = Arc::clone(&stop);
    let stop_send = Arc::clone(&stop);
    let capture = std::thread::spawn(move || capture_loop(tx, stop_cap));
    let send_result = tokio::runtime::Handle::current().block_on(async move {
        use pjdesk_proto::peer::{write_side_chunk, write_side_header, SideHeader, AUDIO_MAGIC};
        let mut uni = conn.open_uni().await?;
        write_side_header(&mut uni, AUDIO_MAGIC, &SideHeader { id: 0, offset: 0, a: 0, b: 0 }).await?;
        while !stop_send.load(Ordering::SeqCst) {
            match tokio::time::timeout(std::time::Duration::from_millis(250), rx.recv()).await {
                Ok(Some(frame)) => write_side_chunk(&mut uni, &frame).await?,
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        write_side_chunk(&mut uni, &[]).await?;
        let _ = uni.finish();
        Ok(())
    });
    stop.store(true, Ordering::SeqCst);
    let _ = capture.join();
    send_result
}

/// Plays 48 kHz stereo s16 frames. `None` if the device cannot be opened.
pub fn start_player() -> Option<Player> {
    #[cfg(windows)]
    {
        Player::spawn()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub struct Player {
    tx: Option<std::sync::mpsc::Sender<Vec<i16>>>,
}

impl Player {
    pub fn push(&self, pcm_le: &[u8]) {
        let Some(tx) = &self.tx else { return };
        let mut samples = Vec::with_capacity(pcm_le.len() / 2);
        for chunk in pcm_le.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        let _ = tx.send(samples);
    }
}

#[cfg(windows)]
fn capture_wasapi(tx: mpsc::Sender<Vec<u8>>, stop: Arc<AtomicBool>) -> Result<()> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        if format.is_null() {
            bail!("the audio device has no mix format");
        }
        let mix = &*format;
        let channels = mix.nChannels as usize;
        let rate = mix.nSamplesPerSec;
        let bits = mix.wBitsPerSample;
        let tag = mix.wFormatTag;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            20_000_000,
            0,
            format,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let mut pending: Vec<i16> = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let packet = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            let mut packet = packet;
            while packet > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if frames > 0 && !data.is_null() && flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) == 0 {
                    let samples = frames as usize * channels;
                    let pcm = decode_mix(data, samples, bits, tag);
                    let stereo = to_stereo(&pcm, channels);
                    let resampled = resample_s16_interleaved(&stereo, AUDIO_CHANNELS as usize, rate, AUDIO_RATE);
                    pending.extend_from_slice(&resampled);
                    while pending.len() >= AUDIO_FRAME_SAMPLES {
                        let frame: Vec<i16> = pending.drain(..AUDIO_FRAME_SAMPLES).collect();
                        let bytes: Vec<u8> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
                        if tx.blocking_send(bytes).is_err() {
                            let _ = capture.ReleaseBuffer(frames);
                            let _ = client.Stop();
                            return Ok(());
                        }
                    }
                }
                capture.ReleaseBuffer(frames)?;
                packet = match capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break,
                };
            }
        }
        let _ = client.Stop();
        Ok(())
    }
}

#[cfg(windows)]
fn decode_mix(data: *mut u8, samples: usize, bits: u16, tag: u16) -> Vec<i16> {
    unsafe {
        // WAVE_FORMAT_IEEE_FLOAT = 3, WAVE_FORMAT_EXTENSIBLE = 0xFFFE.
        // Extensible float is the common mix format; treat 32-bit as float
        // and 16-bit as PCM.
        if bits == 16 {
            let p = data as *const i16;
            std::slice::from_raw_parts(p, samples).to_vec()
        } else if bits == 32 || tag == 3 || tag == 0xFFFE {
            let p = data as *const f32;
            std::slice::from_raw_parts(p, samples).iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
        } else {
            vec![0; samples]
        }
    }
}

#[cfg(windows)]
fn to_stereo(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    if channels == 2 {
        return samples.to_vec();
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let l = samples[i * channels];
        let r = if channels > 1 { samples[i * channels + 1] } else { l };
        out.push(l);
        out.push(r);
    }
    out
}

#[cfg(windows)]
impl Player {
    fn spawn() -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<i16>>();
        std::thread::Builder::new().name("audio-play".into()).spawn(move || play_wasapi(rx)).ok()?;
        Some(Self { tx: Some(tx) })
    }
}

#[cfg(windows)]
fn play_wasapi(rx: std::sync::mpsc::Receiver<Vec<i16>>) {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    let _ = play_inner(rx);
    fn play_inner(rx: std::sync::mpsc::Receiver<Vec<i16>>) -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            if format.is_null() {
                bail!("no mix format");
            }
            let mix = &*format;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 20_000_000, 0, format, None)?;
            let render: IAudioRenderClient = client.GetService()?;
            let buffer = client.GetBufferSize()?;
            client.Start()?;
            let channels = mix.nChannels as usize;
            let rate = mix.nSamplesPerSec;
            let bits = mix.wBitsPerSample;
            let mut queued: Vec<i16> = Vec::new();
            loop {
                let padding = client.GetCurrentPadding()?;
                let want = buffer.saturating_sub(padding) as usize;
                while let Ok(frame) = rx.try_recv() {
                    queued.extend_from_slice(&frame);
                }
                if want == 0 {
                    if rx.recv_timeout(std::time::Duration::from_millis(20)).is_err() && queued.is_empty() {
                        // The sender hung up, or we are just idle. Keep going
                        // until the channel disconnects and the queue is empty.
                        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                            Ok(frame) => queued.extend_from_slice(&frame),
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(_) => continue,
                        }
                    }
                    continue;
                }
                if queued.len() < channels.max(1) {
                    match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                        Ok(frame) => queued.extend_from_slice(&frame),
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) if queued.is_empty() => break,
                        Err(_) => continue,
                    }
                }
                let data = render.GetBuffer(want as u32)?;
                write_mix(data, want, channels, rate, bits, &mut queued);
                render.ReleaseBuffer(want as u32, 0)?;
            }
            let _ = client.Stop();
            Ok(())
        }
    }
}

#[cfg(windows)]
fn write_mix(data: *mut u8, frames: usize, channels: usize, rate: u32, bits: u16, queued: &mut Vec<i16>) {
    if channels == 0 || data.is_null() {
        return;
    }
    let stereo = if rate == AUDIO_RATE {
        std::mem::take(queued)
    } else {
        let got = std::mem::take(queued);
        resample_s16_interleaved(&got, 2, AUDIO_RATE, rate)
    };
    let mut src = stereo;
    unsafe {
        if bits == 16 {
            let dst = std::slice::from_raw_parts_mut(data as *mut i16, frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    let sample = src.get(frame * 2 + ch.min(1)).copied().unwrap_or(0);
                    dst[frame * channels + ch] = sample;
                }
            }
            let used = (frames * 2).min(src.len());
            src.drain(..used);
        } else {
            let dst = std::slice::from_raw_parts_mut(data as *mut f32, frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    let sample = src.get(frame * 2 + ch.min(1)).copied().unwrap_or(0);
                    dst[frame * channels + ch] = sample as f32 / i16::MAX as f32;
                }
            }
            let used = (frames * 2).min(src.len());
            src.drain(..used);
        }
    }
    *queued = src;
}

#[cfg(not(windows))]
impl Player {
    #[allow(dead_code)]
    fn unused() {}
}
