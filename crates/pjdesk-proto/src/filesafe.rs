//! Safe destination paths for a file transfer.
//!
//! Received files are written only under a directory the person at the
//! computer chose. The remote name is one path component: no `..`, no
//! separators, no Windows device names, no alternate data streams. An existing
//! symlink or junction at that name is refused, so a transfer cannot be aimed
//! outside the directory. Nothing here executes the file.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
/// The end-of-transfer check uses SHA-256; callers stream bytes through this.
pub use sha2::{Digest, Sha256};

/// Largest single transfer accepted (8 GiB).
pub const MAX_TRANSFER_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// `resume_from` in an offer that means "keep whatever part you already have".
/// Sent when a transfer is offered again after the connection came back.
/// An older receiver sees a mismatch and simply starts from 0.
pub const RESUME_PARTIAL: u64 = u64::MAX;

/// Where the receiver continues an offered file, given what it already holds.
pub fn resume_offset(resume_from: u64, have: u64, size: u64) -> u64 {
    if resume_from == have || (resume_from == RESUME_PARTIAL && have <= size) {
        have
    } else {
        0
    }
}
/// Suffix for a transfer that has not finished. Removed on success.
pub const PARTIAL_SUFFIX: &str = ".pjpart";

/// Turns a remote file name into a single safe path component.
pub fn sanitize_name(name: &str) -> Result<String> {
    if name != name.trim() {
        bail!("file name must not start or end with a space");
    }
    let name = name.trim();
    if name.is_empty() || name.len() > 180 {
        bail!("file name is empty or too long");
    }
    if name.contains('\0') || name.chars().any(|c| c.is_control()) {
        bail!("file name contains a control character");
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        bail!("file name must not contain a path");
    }
    if name == "." || name == ".." || name.starts_with('.') && name.chars().all(|c| c == '.') {
        bail!("file name is not allowed");
    }
    // Windows rejects trailing dots and spaces, and they can confuse a later rename.
    if name.ends_with('.') || name.ends_with(' ') {
        bail!("file name must not end with a dot or space");
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved(stem) {
        bail!("file name is reserved on Windows");
    }
    Ok(name.to_string())
}

fn is_reserved(stem: &str) -> bool {
    let s = stem.to_ascii_uppercase();
    matches!(
        s.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}

/// `root/name` after checking the name. Does not touch the filesystem.
pub fn join_under(root: &Path, name: &str) -> Result<PathBuf> {
    let name = sanitize_name(name)?;
    if name.ends_with(PARTIAL_SUFFIX) || name.ends_with(META_SUFFIX) {
        bail!("that name is reserved for an unfinished transfer");
    }
    Ok(root.join(name))
}

fn refuse_reparse(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("checking {}", path.display())),
    };
    if meta.file_type().is_symlink() {
        bail!("{} is a link; refusing to follow it out of the receive folder", path.display());
    }
    // A directory where a file should be is also a way out (junctions that
    // Rust does not report as symlinks still show up as directories).
    if meta.is_dir() {
        bail!("{} is a directory; refusing to write into it", path.display());
    }
    Ok(())
}

/// Opens `root/name` for writing, optionally continuing a partial file.
///
/// Returns the file and the offset the sender should continue from. When
/// `resume_from` does not match the bytes already on disk, the partial file
/// is truncated and the offset is 0.
pub fn open_for_receive(root: &Path, name: &str, resume_from: u64) -> Result<(File, u64)> {
    if !root.is_dir() {
        bail!("choose a folder to receive files into before transferring");
    }
    let final_path = join_under(root, name)?;
    let partial = partial_path(&final_path);
    refuse_reparse(&final_path)?;
    refuse_reparse(&partial)?;

    if final_path.exists() && resume_from == 0 {
        // A finished file of the same name is replaced only from the start,
        // and only if it is a regular file (checked above).
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .truncate(false)
        .open(&partial)
        .with_context(|| format!("creating {}", partial.display()))?;
    // Refuse if the path changed identity under us (a link swapped in).
    refuse_reparse(&partial)?;
    // Received files may land straight in Downloads: keep the partial out of sight.
    set_hidden(&partial, true);

    let len = file.metadata()?.len();
    let offset = if resume_from > 0 && resume_from == len && len > 0 {
        file.seek(SeekFrom::Start(len))?;
        len
    } else {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        0
    };
    // A received file is data, not something to execute. On Unix, drop the
    // executable bits. Windows has no equivalent that we set.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o644));
    }
    Ok((file, offset))
}

pub fn partial_path(final_path: &Path) -> PathBuf {
    let mut name = final_path.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    name.push(PARTIAL_SUFFIX);
    final_path.with_file_name(name)
}

/// Renames a finished partial file onto its real name. Refuses if either
/// path is a link.
pub fn finish_receive(root: &Path, name: &str) -> Result<PathBuf> {
    let final_path = join_under(root, name)?;
    let partial = partial_path(&final_path);
    refuse_reparse(&partial)?;
    refuse_reparse(&final_path)?;
    if final_path.exists() {
        fs::remove_file(&final_path)?;
    }
    fs::rename(&partial, &final_path).with_context(|| format!("saving {}", final_path.display()))?;
    refuse_reparse(&final_path)?;
    // The rename keeps the partial's hidden attribute. The finished file shows.
    set_hidden(&final_path, false);
    let _ = fs::remove_file(meta_path(&final_path));
    Ok(final_path)
}

/// `SideHeader.a` bit on a file stream: a SHA-256 of the whole file (32 bytes)
/// follows the end chunk. Older receivers stop reading at the end chunk and
/// never see it; older senders leave the bit clear and nothing is checked.
pub const HASH_TRAILER: u32 = 1;
/// Sidecar next to a partial file, recording what it is a part of.
pub const META_SUFFIX: &str = ".pjpart.meta";
/// Partial files untouched for this long are removed.
pub const STALE_PARTIAL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct PartialMeta {
    size: u64,
    #[serde(default)]
    started_unix: u64,
}

pub fn meta_path(final_path: &Path) -> PathBuf {
    let mut name = final_path.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    name.push(META_SUFFIX);
    final_path.with_file_name(name)
}

/// Bytes of `name` this side can continue from, for a file of `size` bytes.
/// 0 when there is no partial, it is longer than the file, or its sidecar
/// says it belongs to a file of a different size. A partial without a sidecar
/// (left by an older version) is trusted on length alone; the end-of-transfer
/// hash still catches a mismatch when both sides support it.
pub fn resumable_len(root: &Path, name: &str, size: u64) -> u64 {
    let have = partial_len(root, name).unwrap_or(0);
    if have == 0 || have > size {
        return 0;
    }
    let Ok(final_path) = join_under(root, name) else { return 0 };
    match fs::read(meta_path(&final_path)).ok().and_then(|b| serde_json::from_slice::<PartialMeta>(&b).ok()) {
        Some(meta) if meta.size != size => 0,
        _ => have,
    }
}

/// Records that the partial of `name` is part of a `size`-byte file.
pub fn note_partial(root: &Path, name: &str, size: u64) -> Result<()> {
    let final_path = join_under(root, name)?;
    let meta = meta_path(&final_path);
    refuse_reparse(&meta)?;
    let started_unix =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // CREATE_ALWAYS fails on a hidden file, so show it before rewriting.
    set_hidden(&meta, false);
    fs::write(&meta, serde_json::to_vec(&PartialMeta { size, started_unix })?)?;
    set_hidden(&meta, true);
    Ok(())
}

/// SHA-256 state over the first `len` bytes of `path`.
pub fn hash_prefix(path: &Path, len: u64) -> Result<sha2::Sha256> {
    use sha2::Digest;
    use std::io::Read;
    let mut hasher = sha2::Sha256::new();
    let mut file = File::open(path)?.take(len);
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
    }
    if done != len {
        bail!("{} is shorter than expected", path.display());
    }
    Ok(hasher)
}

/// Like [`finish_receive`], first checking the whole partial file against the
/// sender's SHA-256 when it sent one. A mismatch removes the partial file so a
/// retry starts clean.
pub fn finish_receive_checked(root: &Path, name: &str, expected: Option<[u8; 32]>) -> Result<PathBuf> {
    if let Some(want) = expected {
        use sha2::Digest;
        let final_path = join_under(root, name)?;
        let partial = partial_path(&final_path);
        refuse_reparse(&partial)?;
        let len = fs::metadata(&partial)?.len();
        let got: [u8; 32] = hash_prefix(&partial, len)?.finalize().into();
        if got != want {
            let _ = fs::remove_file(&partial);
            let _ = fs::remove_file(meta_path(&final_path));
            bail!("the file arrived damaged (checksum mismatch); Retry sends it again");
        }
    }
    finish_receive(root, name)
}

/// Removes partial files (and their sidecars) in `root` not written for
/// `max_age`. Returns how many partial files went.
pub fn sweep_stale(root: &Path, max_age: std::time::Duration) -> usize {
    let Ok(dir) = fs::read_dir(root) else { return 0 };
    let mut gone = 0;
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_part = name.ends_with(PARTIAL_SUFFIX);
        if !is_part && !name.ends_with(META_SUFFIX) {
            continue;
        }
        let Ok(meta) = entry.path().symlink_metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let old = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age >= max_age);
        if !old {
            continue;
        }
        if is_part {
            // Keep a sidecar only as long as its partial.
            if fs::remove_file(entry.path()).is_ok() {
                gone += 1;
                let mut side = entry.path().into_os_string();
                side.push(".meta");
                let _ = fs::remove_file(PathBuf::from(side));
            }
        } else {
            let base = name.trim_end_matches(".meta");
            if !root.join(base).exists() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    gone
}

/// How many bytes of `path` are already received, if a partial file exists
/// and is a regular file.
pub fn partial_len(root: &Path, name: &str) -> Result<u64> {
    let final_path = join_under(root, name)?;
    let partial = partial_path(&final_path);
    match fs::symlink_metadata(&partial) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => Ok(m.len()),
        Ok(_) => bail!("unfinished transfer is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reoffer_after_reconnect_keeps_the_partial_file() {
        assert_eq!(super::resume_offset(super::RESUME_PARTIAL, 1_000_000_000, 1_100_000_000), 1_000_000_000);
        assert_eq!(super::resume_offset(super::RESUME_PARTIAL, 0, 5), 0);
        assert_eq!(super::resume_offset(0, 1_000, 5_000), 0);
        assert_eq!(super::resume_offset(1_000, 1_000, 5_000), 1_000);
        assert_eq!(super::resume_offset(super::RESUME_PARTIAL, 9_000, 5_000), 0);
        assert_eq!(super::resume_offset(5_000_000_000, 5_000_000_000, 6_000_000_000), 5_000_000_000);
    }

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pjdesk-filesafe-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_paths_and_reserved_names() {
        assert!(sanitize_name("report.pdf").is_ok());
        assert!(sanitize_name("..").is_err());
        assert!(sanitize_name("../x").is_err());
        assert!(sanitize_name("a/b").is_err());
        assert!(sanitize_name("a\\b").is_err());
        assert!(sanitize_name("C:foo").is_err());
        assert!(sanitize_name("CON").is_err());
        assert!(sanitize_name("nul.txt").is_err());
        assert!(sanitize_name("com1.dat").is_err());
        assert!(sanitize_name("foo ").is_err());
        assert!(sanitize_name("").is_err());
        assert!(sanitize_name("has\nline").is_err());
    }

    #[test]
    fn writes_only_inside_the_folder_and_resumes() {
        let root = scratch("resume");
        let (mut file, off) = open_for_receive(&root, "notes.txt", 0).unwrap();
        assert_eq!(off, 0);
        use std::io::Write;
        file.write_all(b"hello").unwrap();
        drop(file);
        assert_eq!(partial_len(&root, "notes.txt").unwrap(), 5);
        let (mut file, off) = open_for_receive(&root, "notes.txt", 5).unwrap();
        assert_eq!(off, 5);
        file.write_all(b" world").unwrap();
        drop(file);
        let saved = finish_receive(&root, "notes.txt").unwrap();
        assert_eq!(fs::read(&saved).unwrap(), b"hello world");
        assert!(!partial_path(&saved).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mismatched_resume_starts_over() {
        let root = scratch("mismatch");
        let (mut file, _) = open_for_receive(&root, "a.bin", 0).unwrap();
        use std::io::Write;
        file.write_all(b"abcdef").unwrap();
        drop(file);
        let (file, off) = open_for_receive(&root, "a.bin", 3).unwrap();
        assert_eq!(off, 0);
        assert_eq!(file.metadata().unwrap().len(), 0);
        drop(file);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_partial_for_a_different_file_size_is_not_resumed() {
        let root = scratch("meta");
        let (mut file, _) = open_for_receive(&root, "big.bin", 0).unwrap();
        use std::io::Write;
        file.write_all(&[7u8; 1000]).unwrap();
        drop(file);
        // No sidecar (older version): trusted on length.
        assert_eq!(resumable_len(&root, "big.bin", 5000), 1000);
        note_partial(&root, "big.bin", 5000).unwrap();
        assert_eq!(resumable_len(&root, "big.bin", 5000), 1000);
        assert_eq!(resumable_len(&root, "big.bin", 6000), 0);
        assert_eq!(resumable_len(&root, "big.bin", 500), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_file_is_refused_and_a_good_one_saved() {
        use sha2::Digest;
        let root = scratch("hash");
        let body = b"0123456789abcdef".repeat(1000);
        let want: [u8; 32] = sha2::Sha256::digest(&body).into();
        for (damaged, name) in [(true, "x.bin"), (false, "y.bin")] {
            let (mut file, _) = open_for_receive(&root, name, 0).unwrap();
            use std::io::Write;
            let mut data = body.clone();
            if damaged {
                data[5000] ^= 1;
            }
            file.write_all(&data).unwrap();
            drop(file);
            note_partial(&root, name, data.len() as u64).unwrap();
            let r = finish_receive_checked(&root, name, Some(want));
            assert_eq!(r.is_err(), damaged);
            assert_eq!(partial_len(&root, name).unwrap(), 0);
            assert!(!meta_path(&root.join(name)).exists());
        }
        assert_eq!(fs::read(root.join("y.bin")).unwrap(), body);
        assert!(!root.join("x.bin").exists());
        // The prefix hash continues to the same digest.
        let mut h = hash_prefix(&root.join("y.bin"), 4000).unwrap();
        h.update(&body[4000..]);
        assert_eq!(<[u8; 32]>::from(h.finalize()), want);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_partials_are_swept_and_fresh_ones_kept() {
        let root = scratch("sweep");
        fs::write(root.join("a.bin.pjpart"), b"x").unwrap();
        fs::write(root.join("a.bin.pjpart.meta"), b"{}").unwrap();
        fs::write(root.join("keep.txt"), b"x").unwrap();
        assert_eq!(sweep_stale(&root, std::time::Duration::from_secs(3600)), 0);
        assert!(root.join("a.bin.pjpart").exists());
        assert_eq!(sweep_stale(&root, std::time::Duration::ZERO), 1);
        assert!(!root.join("a.bin.pjpart").exists());
        assert!(!root.join("a.bin.pjpart.meta").exists());
        assert!(root.join("keep.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_in_the_receive_folder() {
        let root = scratch("link");
        let outside = scratch("outside");
        std::os::unix::fs::symlink(outside.join("secret"), root.join("notes.txt.pjpart")).unwrap();
        assert!(open_for_receive(&root, "notes.txt", 0).is_err());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}

/// Sets or clears the Windows hidden attribute. No-op elsewhere (the partial
/// name already ends in a suffix file managers sort apart).
pub fn set_hidden(path: &Path, hidden: bool) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileAttributesW(name: *const u16) -> u32;
            fn SetFileAttributesW(name: *const u16, attrs: u32) -> i32;
        }
        const HIDDEN: u32 = 0x2;
        const NORMAL: u32 = 0x80;
        const INVALID: u32 = u32::MAX;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            let attrs = GetFileAttributesW(wide.as_ptr());
            if attrs == INVALID {
                return;
            }
            let mut next = if hidden { attrs | HIDDEN } else { attrs & !HIDDEN };
            if next == 0 {
                next = NORMAL;
            }
            if next != attrs {
                SetFileAttributesW(wide.as_ptr(), next);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, hidden);
    }
}
