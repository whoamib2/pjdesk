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
    if name.ends_with(PARTIAL_SUFFIX) {
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
    Ok(final_path)
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
