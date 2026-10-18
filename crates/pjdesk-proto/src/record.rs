//! Session recording as a single-track H.264 MP4.
//!
//! The controller already has the Annex-B bitstream. This writes a file VLC
//! and the Films & TV app can open: `ftyp`, a 32-bit `mdat` of length-prefixed
//! samples, then `moov`. Recording stops near 1.5 GiB because the `mdat` size
//! field is 32 bits. The file is only the picture the controller was allowed
//! to see; starting it still requires the host's recording permission.

use std::io::{Seek, SeekFrom, Write};

use anyhow::{bail, Result};

const MAX_MDAT: u64 = 1500 * 1024 * 1024;

struct Sample {
    size: u32,
    offset: u32,
    key: bool,
}

pub struct Recorder<W: Write + Seek> {
    out: W,
    mdat_size_at: u64,
    mdat_data_at: u64,
    written: u64,
    samples: Vec<Sample>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    width: u16,
    height: u16,
    frame_ms: u32,
    stopped: bool,
}

impl<W: Write + Seek> Recorder<W> {
    pub fn start(mut out: W, width: u16, height: u16, fps: u32) -> Result<Self> {
        out.write_all(&box_header(24, *b"ftyp"))?;
        out.write_all(b"isom")?;
        out.write_all(&0u32.to_be_bytes())?;
        out.write_all(b"isomiso2avc1mp41")?;
        // mdat with a placeholder size. Large-file (64-bit) size is avoided so
        // the layout stays simple; we refuse to pass MAX_MDAT.
        let mdat_size_at = out.stream_position()?;
        out.write_all(&box_header(8, *b"mdat"))?;
        let mdat_data_at = out.stream_position()?;
        let frame_ms = (1000 / fps.clamp(1, 60)).max(1);
        Ok(Self {
            out,
            mdat_size_at,
            mdat_data_at,
            written: 0,
            samples: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
            width: width.max(16),
            height: height.max(16),
            frame_ms,
            stopped: false,
        })
    }

    /// Appends one Annex-B access unit. The first keyframe must contain SPS and PPS.
    pub fn push(&mut self, annex_b: &[u8], keyframe: bool) -> Result<()> {
        if self.stopped {
            bail!("recording is already finished");
        }
        let nals = split_annex_b(annex_b);
        if nals.is_empty() {
            return Ok(());
        }
        for nal in &nals {
            match nal.first().copied().unwrap_or(0) & 0x1f {
                7 => self.sps = nal.clone(),
                8 => self.pps = nal.clone(),
                _ => {}
            }
        }
        if self.sps.is_empty() || self.pps.is_empty() {
            // Wait for a keyframe that carries the decoder config.
            return Ok(());
        }
        let mut sample = Vec::new();
        for nal in &nals {
            let kind = nal.first().copied().unwrap_or(0) & 0x1f;
            if kind == 7 || kind == 8 || nal.is_empty() {
                continue;
            }
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        if sample.is_empty() {
            return Ok(());
        }
        if self.written + sample.len() as u64 > MAX_MDAT {
            bail!("recording reached its size limit");
        }
        let offset = (self.mdat_data_at + self.written) as u32;
        self.out.write_all(&sample)?;
        self.written += sample.len() as u64;
        self.samples.push(Sample { size: sample.len() as u32, offset, key: keyframe });
        Ok(())
    }

    pub fn frames(&self) -> usize {
        self.samples.len()
    }

    /// Patches the `mdat` size and writes `moov`. Returns the sample count.
    pub fn finish(mut self) -> Result<usize> {
        if self.samples.is_empty() || self.sps.len() < 4 || self.pps.is_empty() {
            bail!("recording has no playable frames");
        }
        let count = self.samples.len();
        let mdat_box = (8 + self.written) as u32;
        let end = self.out.stream_position()?;
        self.out.seek(SeekFrom::Start(self.mdat_size_at))?;
        self.out.write_all(&mdat_box.to_be_bytes())?;
        self.out.seek(SeekFrom::Start(end))?;
        self.write_moov()?;
        self.out.flush()?;
        self.stopped = true;
        Ok(count)
    }

    fn write_moov(&mut self) -> Result<()> {
        let duration = self.samples.len() as u32 * self.frame_ms;
        let stbl = self.stbl()?;
        let minf = children(*b"minf", &[vmhd(), dinf(), stbl])?;
        let mdhd = mdhd(duration);
        let hdlr = hdlr();
        let mdia = children(*b"mdia", &[mdhd, hdlr, minf])?;
        let tkhd = tkhd(duration, self.width, self.height);
        let trak = children(*b"trak", &[tkhd, mdia])?;
        let mvhd = mvhd(duration);
        let moov = children(*b"moov", &[mvhd, trak])?;
        self.out.write_all(&moov)?;
        Ok(())
    }

    fn stbl(&self) -> Result<Vec<u8>> {
        let stsd = stsd(&self.sps, &self.pps, self.width, self.height);
        let mut stts = Vec::new();
        stts.extend_from_slice(&box_header(16 + 8, *b"stts"));
        stts.extend_from_slice(&0u32.to_be_bytes());
        stts.extend_from_slice(&1u32.to_be_bytes());
        stts.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
        stts.extend_from_slice(&self.frame_ms.to_be_bytes());

        let mut stsc = Vec::new();
        stsc.extend_from_slice(&box_header(28, *b"stsc"));
        stsc.extend_from_slice(&0u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes()); // first chunk
        stsc.extend_from_slice(&1u32.to_be_bytes()); // samples per chunk
        stsc.extend_from_slice(&1u32.to_be_bytes()); // description

        // One chunk per sample keeps offsets obvious. stsc above says 1 sample
        // per chunk, so we need one stco entry per sample and a matching stsc
        // that only describes the first chunk when every chunk is identical.
        // ISO says later chunks repeat the last stsc entry. One entry is correct.

        let mut stsz = Vec::new();
        let stsz_size = 20 + 4 * self.samples.len() as u32;
        stsz.extend_from_slice(&box_header(stsz_size, *b"stsz"));
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&0u32.to_be_bytes()); // default size
        stsz.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
        for s in &self.samples {
            stsz.extend_from_slice(&s.size.to_be_bytes());
        }

        let mut stco = Vec::new();
        let stco_size = 16 + 4 * self.samples.len() as u32;
        stco.extend_from_slice(&box_header(stco_size, *b"stco"));
        stco.extend_from_slice(&0u32.to_be_bytes());
        stco.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
        for s in &self.samples {
            stco.extend_from_slice(&s.offset.to_be_bytes());
        }

        let keys: Vec<&Sample> = self.samples.iter().filter(|s| s.key).collect();
        let mut stss = Vec::new();
        let stss_size = 16 + 4 * keys.len().max(1) as u32;
        stss.extend_from_slice(&box_header(stss_size, *b"stss"));
        stss.extend_from_slice(&0u32.to_be_bytes());
        if keys.is_empty() {
            stss.extend_from_slice(&1u32.to_be_bytes());
            stss.extend_from_slice(&1u32.to_be_bytes());
        } else {
            stss.extend_from_slice(&(keys.len() as u32).to_be_bytes());
            for (i, s) in self.samples.iter().enumerate() {
                if s.key {
                    stss.extend_from_slice(&((i as u32) + 1).to_be_bytes());
                }
            }
        }
        children(*b"stbl", &[stsd, stts, stsc, stsz, stco, stss])
    }
}

fn box_header(size: u32, kind: [u8; 4]) -> [u8; 8] {
    let mut h = [0u8; 8];
    h[..4].copy_from_slice(&size.to_be_bytes());
    h[4..].copy_from_slice(&kind);
    h
}

fn children(kind: [u8; 4], parts: &[Vec<u8>]) -> Result<Vec<u8>> {
    let payload: usize = parts.iter().map(|p| p.len()).sum();
    let size = 8 + payload;
    if size > u32::MAX as usize {
        bail!("recording header is too large");
    }
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&box_header(size as u32, kind));
    for p in parts {
        out.extend_from_slice(p);
    }
    Ok(out)
}

fn full_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + payload.len());
    out.extend_from_slice(&box_header(12 + payload.len() as u32, kind));
    out.extend_from_slice(&0u32.to_be_bytes()); // version + flags
    out.extend_from_slice(payload);
    out
}

fn mvhd(duration: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(96);
    p.extend_from_slice(&0u32.to_be_bytes()); // creation
    p.extend_from_slice(&0u32.to_be_bytes()); // modification
    p.extend_from_slice(&1000u32.to_be_bytes());
    p.extend_from_slice(&duration.to_be_bytes());
    p.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate 1.0
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&[0u8; 8]);
    // unity matrix
    for v in [0x00010000, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000u32] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&[0u8; 24]); // pre_defined
    p.extend_from_slice(&2u32.to_be_bytes()); // next track
    full_box(*b"mvhd", &p)
}

fn tkhd(duration: u32, width: u16, height: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&1u32.to_be_bytes()); // track id
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&duration.to_be_bytes());
    p.extend_from_slice(&[0u8; 8]);
    p.extend_from_slice(&0u16.to_be_bytes()); // layer
    p.extend_from_slice(&0u16.to_be_bytes()); // alternate group
    p.extend_from_slice(&0u16.to_be_bytes()); // volume (video)
    p.extend_from_slice(&0u16.to_be_bytes());
    for v in [0x00010000, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000u32] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&(u32::from(width) << 16).to_be_bytes());
    p.extend_from_slice(&(u32::from(height) << 16).to_be_bytes());
    // flags: track enabled + in movie + in preview
    let mut box_bytes = full_box(*b"tkhd", &p);
    box_bytes[8] = 0;
    box_bytes[11] = 0x07;
    box_bytes
}

fn mdhd(duration: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&1000u32.to_be_bytes());
    p.extend_from_slice(&duration.to_be_bytes());
    p.extend_from_slice(&0x55c4u16.to_be_bytes()); // und
    p.extend_from_slice(&0u16.to_be_bytes());
    full_box(*b"mdhd", &p)
}

fn hdlr() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(b"vide");
    p.extend_from_slice(&[0u8; 12]);
    p.extend_from_slice(b"PJDesk Video\0");
    full_box(*b"hdlr", &p)
}

fn vmhd() -> Vec<u8> {
    let mut b = full_box(*b"vmhd", &[0, 0, 0, 0, 0, 0, 0, 0]);
    b[11] = 1; // flags
    b
}

fn dinf() -> Vec<u8> {
    let mut dref_entry = Vec::new();
    dref_entry.extend_from_slice(&box_header(12, *b"url "));
    dref_entry.extend_from_slice(&1u32.to_be_bytes()); // flags: self-contained
    let mut dref = Vec::new();
    dref.extend_from_slice(&box_header(8 + 8 + dref_entry.len() as u32, *b"dref"));
    dref.extend_from_slice(&0u32.to_be_bytes());
    dref.extend_from_slice(&1u32.to_be_bytes());
    dref.extend_from_slice(&dref_entry);
    children(*b"dinf", &[dref]).unwrap_or_default()
}

fn stsd(sps: &[u8], pps: &[u8], width: u16, height: u16) -> Vec<u8> {
    let avcc = avcc(sps, pps);
    let mut avc1 = Vec::new();
    let avc1_size = 8 + 78 + avcc.len();
    avc1.extend_from_slice(&box_header(avc1_size as u32, *b"avc1"));
    avc1.extend_from_slice(&[0u8; 6]);
    avc1.extend_from_slice(&1u16.to_be_bytes()); // data reference
    avc1.extend_from_slice(&[0u8; 16]);
    avc1.extend_from_slice(&width.to_be_bytes());
    avc1.extend_from_slice(&height.to_be_bytes());
    avc1.extend_from_slice(&0x00480000u32.to_be_bytes());
    avc1.extend_from_slice(&0x00480000u32.to_be_bytes());
    avc1.extend_from_slice(&0u32.to_be_bytes());
    avc1.extend_from_slice(&1u16.to_be_bytes()); // frame count
    avc1.extend_from_slice(&[0u8; 32]); // compressor name
    avc1.extend_from_slice(&0x0018u16.to_be_bytes());
    avc1.extend_from_slice(&(-1i16).to_be_bytes());
    avc1.extend_from_slice(&avcc);
    let mut stsd = Vec::new();
    stsd.extend_from_slice(&box_header(16 + avc1.len() as u32, *b"stsd"));
    stsd.extend_from_slice(&0u32.to_be_bytes());
    stsd.extend_from_slice(&1u32.to_be_bytes());
    stsd.extend_from_slice(&avc1);
    stsd
}

fn avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(1);
    p.push(sps.get(1).copied().unwrap_or(0x42));
    p.push(sps.get(2).copied().unwrap_or(0));
    p.push(sps.get(3).copied().unwrap_or(0x1e));
    p.push(0xff);
    p.push(0xe1);
    p.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    p.extend_from_slice(sps);
    p.push(1);
    p.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    p.extend_from_slice(pps);
    let mut b = Vec::new();
    b.extend_from_slice(&box_header(8 + p.len() as u32, *b"avcC"));
    b.extend_from_slice(&p);
    b
}

/// Splits an Annex-B bitstream into NAL units (no start codes).
pub fn split_annex_b(data: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
            continue;
        }
        if i + 4 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
            starts.push(i + 4);
            i += 4;
            continue;
        }
        i += 1;
    }
    let mut nals = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(data.len());
        let mut end = end;
        // Drop the next start code's leading zeros that we included.
        if n + 1 < starts.len() {
            let mut back = end;
            while back > start && data[back - 1] == 0 {
                back -= 1;
            }
            // The start-code finder points at the byte after 00 00 01, so the
            // zeros before it belong to the start code, not the NAL.
            end = back;
        }
        if end > start {
            nals.push(data[start..end].to_vec());
        }
    }
    nals
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn annex(nals: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            v.extend_from_slice(&[0, 0, 0, 1]);
            v.extend_from_slice(n);
        }
        v
    }

    #[test]
    fn writes_a_playable_box_structure() {
        let sps = [0x67, 0x42, 0x00, 0x1e, 0xaa];
        let pps = [0x68, 0xce, 0x38, 0x80];
        let idr = [0x65, 0x88, 0x84, 0x00, 0x11];
        let pframe = [0x41, 0x9a, 0x00, 0x22];
        let mut rec = Recorder::start(Cursor::new(Vec::new()), 320, 240, 30).unwrap();
        rec.push(&annex(&[&sps, &pps, &idr]), true).unwrap();
        rec.push(&annex(&[&pframe]), false).unwrap();
        assert_eq!(rec.frames(), 2);
        let n = rec.finish().unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn finish_without_sps_fails() {
        let mut rec = Recorder::start(Cursor::new(Vec::new()), 64, 64, 30).unwrap();
        rec.push(&annex(&[&[0x41, 0x00, 0x01]]), false).unwrap();
        assert!(rec.finish().is_err());
    }

    #[test]
    fn boxes_are_present() {
        let sps = [0x67, 0x42, 0x00, 0x1e];
        let pps = [0x68, 0xce, 0x38];
        let idr = [0x65, 0x00, 0x01];
        let path = std::env::temp_dir().join(format!("pjdesk-rec-{}.mp4", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let mut rec = Recorder::start(file, 160, 120, 10).unwrap();
        rec.push(&annex(&[&sps, &pps, &idr]), true).unwrap();
        rec.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(bytes.windows(4).any(|w| w == b"ftyp"));
        assert!(bytes.windows(4).any(|w| w == b"mdat"));
        assert!(bytes.windows(4).any(|w| w == b"moov"));
        assert!(bytes.windows(4).any(|w| w == b"avcC"));
        assert!(bytes.windows(4).any(|w| w == b"avc1"));
    }
}
