//! Append-only audit log for the server.
//!
//! One JSON object per line in `audit.log`. The line never contains a
//! password or a login token: callers pass a short description, and this
//! module strips newlines so one event cannot become two. The file rotates
//! once past a megabyte. It is local to this server (it is not part of the
//! cloud-sync bundle, which has a fixed set of files).

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pjdesk_proto::signal::AuditEvent;

const MAX_BYTES: u64 = 1024 * 1024;

pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn open(dir: &Path) -> Self {
        Self { path: dir.join("audit.log") }
    }

    pub fn record(&self, kind: &str, actor: &str, detail: &str) {
        let event = AuditEvent {
            at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            kind: clip(kind, 40),
            actor: clip(actor, 64),
            detail: clip(detail, 180),
        };
        let Ok(mut line) = serde_json::to_string(&event) else { return };
        // Defense in depth: a crafted field must not inject a second record.
        line.retain(|c| c != '\n' && c != '\r');
        line.push('\n');
        if let Ok(meta) = std::fs::metadata(&self.path) {
            if meta.len() > MAX_BYTES {
                let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
            }
        }
        let created = !self.path.exists();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = file.write_all(line.as_bytes());
            if created {
                crate::cloud::protect(&self.path);
            }
        }
    }

    /// Newest events last, at most `limit` of them.
    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        let limit = limit.clamp(1, 500);
        let mut lines = read_lines(&self.path.with_extension("log.1"));
        lines.extend(read_lines(&self.path));
        lines.into_iter().rev().take(limit).collect::<Vec<_>>().into_iter().rev().collect()
    }
}

fn clip(s: &str, max: usize) -> String {
    s.replace(['\n', '\r', '\0'], " ").chars().take(max).collect()
}

fn read_lines(path: &Path) -> Vec<AuditEvent> {
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_one_line_and_keep_no_newlines() {
        let dir = std::env::temp_dir().join(format!("pjdesk-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = AuditLog::open(&dir);
        log.record("login_fail", "ada", "wrong password\nsecond line");
        log.record("group", "admin", "123456789 -> Office");
        let events = log.recent(10);
        assert_eq!(events.len(), 2);
        assert!(!events[0].detail.contains('\n'));
        assert!(events[0].detail.contains("wrong password"));
        let raw = std::fs::read_to_string(dir.join("audit.log")).unwrap();
        assert_eq!(raw.lines().count(), 2);
        assert!(!raw.contains("password hash"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
