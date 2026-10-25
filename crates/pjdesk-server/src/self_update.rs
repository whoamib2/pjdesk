//! Optional automatic updates for PJDesk Server.
//!
//! Off until an administrator turns it on. The service then checks about once
//! an hour (30–60 minutes, with a per-server jitter). A standby may install
//! first. A primary that knows a standby is connected waits until that standby
//! reports the new version and the current cloud generation, or until 24 hours
//! have passed, so the two servers do not restart together.
//!
//! The download path is the shared one in `pjdesk-proto`: private directory,
//! required `SHA256SUMS`, and a fresh hash immediately before the installer
//! starts. The installer stops and restarts the service and does not touch the
//! data folder.

use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::Timelike;
use serde::{Deserialize, Serialize};

use crate::cloud::{self, PeerHealth};

const SETTINGS_FILE: &str = "server-update.json";
const STATUS_FILE: &str = "server-update-status.json";
/// How long a primary will wait for a standby before installing anyway.
pub const MAX_WAIT_SECS: u64 = 24 * 60 * 60;
/// A health report older than this is not a standby we should wait for.
const PEER_FRESH_SECS: u64 = 6 * 60 * 60;
#[cfg_attr(not(windows), allow(dead_code))]
pub const UPDATE_TASK: &str = "PJDesk Server Update";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub auto: bool,
    /// Local `HH:MM`, or empty for "any time".
    #[serde(default)]
    pub window_start: String,
    #[serde(default)]
    pub window_end: String,
    /// Added to the 30 minute base so servers do not all check together.
    #[serde(default)]
    pub jitter_secs: u64,
    /// Version we are waiting on, and when we first saw it.
    #[serde(default)]
    pub pending_version: String,
    #[serde(default)]
    pub pending_since: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto: false,
            window_start: String::new(),
            window_end: String::new(),
            jitter_secs: 0,
            pending_version: String::new(),
            pending_since: 0,
        }
    }
}

/// Whether `server-update.json` was read. Unreadable is not the same as
/// "automatic updates are off" — an unelevated window cannot open the file.
#[derive(Debug)]
pub enum Loaded<T> {
    Present(T),
    Absent,
    Unreadable,
}

impl Settings {
    pub fn read(data_dir: &Path) -> Loaded<Self> {
        match std::fs::read(data_dir.join(SETTINGS_FILE)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => Loaded::Present(s),
                Err(_) => Loaded::Present(Self::default()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Absent,
            Err(_) => Loaded::Unreadable,
        }
    }

    pub fn load(data_dir: &Path) -> Self {
        match Self::read(data_dir) {
            Loaded::Unreadable => Self::default(),
            Loaded::Present(mut s) => {
                if s.jitter_secs == 0 {
                    s.jitter_secs = fresh_jitter();
                    s.save(data_dir);
                }
                s
            }
            Loaded::Absent => {
                let mut s = Self::default();
                s.jitter_secs = fresh_jitter();
                s.save(data_dir);
                s
            }
        }
    }

    pub fn save(&self, data_dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::create_dir_all(data_dir);
            let path = data_dir.join(SETTINGS_FILE);
            let _ = std::fs::write(&path, bytes);
            cloud::protect(&path);
        }
    }

    /// Seconds between automatic checks: 30 minutes plus up to 30 minutes.
    pub fn interval_secs(&self) -> u64 {
        30 * 60 + (self.jitter_secs % (30 * 60))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    #[serde(default)]
    pub last_check_at: u64,
    #[serde(default)]
    pub last_result: String,
    #[serde(default)]
    pub available: String,
    #[serde(default)]
    pub error: String,
}

fn fresh_jitter() -> u64 {
    let n = u64::from(pjdesk_proto::random_bytes::<2>()[0]) * 256 + u64::from(pjdesk_proto::random_bytes::<2>()[1]);
    (n % (30 * 60)) + 1
}

impl UpdateStatus {
    fn read_raw(data_dir: &Path) -> Loaded<Self> {
        match std::fs::read(data_dir.join(STATUS_FILE)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => Loaded::Present(s),
                Err(_) => Loaded::Present(Self::default()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Absent,
            Err(_) => Loaded::Unreadable,
        }
    }

    pub fn read(data_dir: &Path) -> Loaded<Self> {
        match Self::read_raw(data_dir) {
            Loaded::Present(s) => Loaded::Present(s.settled(env!("CARGO_PKG_VERSION"))),
            other => other,
        }
    }

    pub fn load(data_dir: &Path) -> Self {
        match Self::read(data_dir) {
            Loaded::Present(s) => s,
            Loaded::Absent | Loaded::Unreadable => Self::default(),
        }
    }

    /// The installer leaves `Installing 0.6.32` in the data folder, then opens
    /// this window. When that version is this copy, the line is already done.
    /// Compared locally: no network check and no service pipe.
    pub fn settled(self, current: &str) -> Self {
        let Some(offered) = self.offered_version() else {
            return self;
        };
        if pjdesk_proto::update::is_newer(current, &offered) {
            return self;
        }
        if self.last_result == "Up to date" && self.available.is_empty() && self.error.is_empty() {
            return self;
        }
        Self { last_check_at: self.last_check_at, last_result: "Up to date".into(), available: String::new(), error: String::new() }
    }

    /// Version named by the status file. The available field wins. Otherwise
    /// `Installing 0.6.32` or `Version 0.6.32 is available`.
    fn offered_version(&self) -> Option<String> {
        let available = self.available.trim().trim_start_matches('v');
        if pjdesk_proto::update::parse_version(available).is_some() {
            return Some(available.to_string());
        }
        if !(self.last_result.starts_with("Installing ") || self.last_result.contains(" is available")) {
            return None;
        }
        self.last_result.split_whitespace().find_map(|token| {
            let token = token.trim_matches(|c: char| matches!(c, '.' | ',' | ')' | '(')).trim_start_matches('v');
            pjdesk_proto::update::parse_version(token).map(|_| token.to_string())
        })
    }

    /// Rewrites a finished install so the next open does not read it again.
    /// A folder this process cannot write is left as it is; [`Self::read`]
    /// still reports the settled line.
    pub fn reconcile(data_dir: &Path) {
        let Loaded::Present(raw) = Self::read_raw(data_dir) else {
            return;
        };
        let settled = raw.clone().settled(env!("CARGO_PKG_VERSION"));
        if settled != raw {
            settled.save(data_dir);
        }
    }

    fn save(&self, data_dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let path = data_dir.join(STATUS_FILE);
            let _ = std::fs::write(&path, bytes);
            cloud::protect(&path);
        }
    }
}

/// What an automatic (or manual) check should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Install,
    /// Try again later. `reason` is a stable short phrase for logs and tests.
    Defer { reason: &'static str },
    Skip { reason: &'static str },
}

/// Inputs to [`decide`]. Times are unix seconds; `minute_of_day` is local.
#[derive(Clone, Debug)]
pub struct Situation {
    pub auto: bool,
    pub manual: bool,
    pub now: u64,
    pub minute_of_day: u16,
    /// `None` means any time. Start inclusive, end exclusive. A window that
    /// wraps past midnight has `start > end`.
    pub window: Option<(u16, u16)>,
    pub current: String,
    pub available: Option<String>,
    /// This server is the cloud primary (it uploads). A server with sync off
    /// is not a primary.
    pub role_active: bool,
    pub our_epoch: u64,
    pub peers: Vec<PeerHealth>,
    pub our_instance: String,
    pub pending_since: u64,
    pub max_wait: u64,
}

pub fn decide(s: &Situation) -> Plan {
    let Some(ver) = s.available.as_deref().filter(|v| !v.is_empty()) else {
        return Plan::Skip { reason: "up to date" };
    };
    if pjdesk_proto::update::is_prerelease(ver) {
        return Plan::Skip { reason: "pre-release" };
    }
    if !pjdesk_proto::update::is_newer(&s.current, ver) {
        return Plan::Skip { reason: "not newer" };
    }
    if s.manual {
        return Plan::Install;
    }
    if !s.auto {
        return Plan::Skip { reason: "automatic updates are off" };
    }
    if !in_window(s.minute_of_day, s.window) {
        return Plan::Defer { reason: "outside the maintenance window" };
    }
    // A standby, or a server that is not the cloud primary, installs first.
    if !s.role_active {
        return Plan::Install;
    }
    let standbys: Vec<&PeerHealth> = s
        .peers
        .iter()
        .filter(|p| p.instance != s.our_instance && s.now.saturating_sub(p.at) <= PEER_FRESH_SECS)
        .collect();
    if standbys.is_empty() {
        return Plan::Install;
    }
    let ready = standbys.iter().any(|p| {
        p.healthy && pjdesk_proto::update::version_at_least(&p.version, ver) && p.epoch >= s.our_epoch
    });
    if ready {
        return Plan::Install;
    }
    if s.pending_since > 0 && s.now.saturating_sub(s.pending_since) >= s.max_wait {
        return Plan::Install;
    }
    Plan::Defer { reason: "waiting for the standby to update" }
}

pub fn in_window(minute: u16, window: Option<(u16, u16)>) -> bool {
    let Some((start, end)) = window else { return true };
    if start == end {
        return true;
    }
    if start < end {
        minute >= start && minute < end
    } else {
        minute >= start || minute < end
    }
}

/// `HH:MM`. Empty is "not set".
pub fn parse_hhmm(text: &str) -> Result<Option<u16>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some((h, m)) = text.split_once(':') else {
        bail!("use HH:MM, or leave the window blank for any time");
    };
    let h: u16 = h.parse().context("use HH:MM, or leave the window blank for any time")?;
    let m: u16 = m.parse().context("use HH:MM, or leave the window blank for any time")?;
    if h > 23 || m > 59 {
        bail!("use HH:MM, or leave the window blank for any time");
    }
    Ok(Some(h * 60 + m))
}

fn window_of(settings: &Settings) -> Result<Option<(u16, u16)>> {
    match (parse_hhmm(&settings.window_start)?, parse_hhmm(&settings.window_end)?) {
        (None, None) => Ok(None),
        (Some(a), Some(b)) => Ok(Some((a, b))),
        _ => bail!("set both the start and the end of the maintenance window, or leave both blank"),
    }
}

pub fn set_auto(data_dir: &Path, auto: bool) -> Settings {
    let mut s = Settings::load(data_dir);
    s.auto = auto;
    s.save(data_dir);
    s
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn set_window(data_dir: &Path, start: &str, end: &str) -> Result<()> {
    let mut s = Settings::load(data_dir);
    s.window_start = start.trim().to_string();
    s.window_end = end.trim().to_string();
    window_of(&s)?;
    s.save(data_dir);
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn minute_of_day() -> u16 {
    let t = chrono::Local::now().time();
    (t.hour() as u16) * 60 + t.minute() as u16
}

fn situation(data_dir: &Path, settings: &Settings, available: Option<String>, manual: bool) -> Situation {
    let status = cloud::CloudStatus::load(data_dir);
    let role_active = status.as_ref().is_some_and(|s| s.enabled && s.role == "active");
    let our_epoch = status.as_ref().map(|s| s.epoch).unwrap_or(0);
    let our_instance = std::fs::read_to_string(data_dir.join("cloud-instance.txt")).unwrap_or_default();
    Situation {
        auto: settings.auto,
        manual,
        now: now_secs(),
        minute_of_day: minute_of_day(),
        window: window_of(settings).unwrap_or(None),
        current: env!("CARGO_PKG_VERSION").into(),
        available,
        role_active,
        our_epoch,
        peers: cloud::peer_health(data_dir),
        our_instance: our_instance.trim().to_string(),
        pending_since: settings.pending_since,
        max_wait: MAX_WAIT_SECS,
    }
}

fn remember_offer(data_dir: &Path, settings: &mut Settings, version: &str) {
    if settings.pending_version != version {
        settings.pending_version = version.to_string();
        settings.pending_since = now_secs();
        settings.save(data_dir);
    }
}

fn record(data_dir: &Path, result: &str, available: &str, error: &str) {
    UpdateStatus {
        last_check_at: now_secs(),
        last_result: result.into(),
        available: available.into(),
        error: error.into(),
    }
    .save(data_dir);
}

/// Checks the release feed and writes the status file. Does not install.
pub fn check_now(data_dir: &Path) -> Result<String> {
    let current = env!("CARGO_PKG_VERSION");
    match pjdesk_proto::update::check_server(current) {
        Ok(None) => {
            record(data_dir, "Up to date", "", "");
            tracing::info!("server update: up to date ({current})");
            Ok(format!("PJDesk Server {current} is up to date."))
        }
        Ok(Some(found)) => {
            record(data_dir, &format!("Version {} is available", found.version), &found.version, "");
            tracing::info!("server update: {} is available (this server is {current})", found.version);
            Ok(format!("PJDesk Server {} is available.", found.version))
        }
        Err(e) => {
            record(data_dir, "Check failed", "", &format!("{e:#}"));
            tracing::warn!("server update: check failed: {e:#}");
            Err(e)
        }
    }
}

/// Checks and, when the plan says so, downloads and launches the installer.
/// `manual` skips the standby wait and the maintenance window.
pub fn install_if_planned(data_dir: &Path, manual: bool) -> Result<String> {
    let current = env!("CARGO_PKG_VERSION");
    let found = match pjdesk_proto::update::check_server(current) {
        Ok(v) => v,
        Err(e) => {
            record(data_dir, "Check failed", "", &format!("{e:#}"));
            tracing::warn!("server update: check failed: {e:#}");
            return Err(e);
        }
    };
    let Some(update) = found else {
        record(data_dir, "Up to date", "", "");
        tracing::info!("server update: up to date ({current})");
        return Ok(format!("PJDesk Server {current} is up to date."));
    };
    let mut settings = Settings::load(data_dir);
    remember_offer(data_dir, &mut settings, &update.version);
    let plan = plan_for(data_dir, &settings, Some(update.version.clone()), manual);
    match plan {
        Plan::Skip { reason } | Plan::Defer { reason } => {
            record(data_dir, reason, &update.version, "");
            tracing::info!("server update: {reason} ({})", update.version);
            Ok(format!("PJDesk Server {} is available ({reason}).", update.version))
        }
        Plan::Install => {
            tracing::info!("server update: installing {}", update.version);
            match pjdesk_proto::update::install_available(&update) {
                Ok(()) => {
                    record(data_dir, &format!("Installing {}", update.version), &update.version, "");
                    // The installer stops this process. If it fails to start,
                    // we are still running and the service was not stopped.
                    Ok(format!("Installing PJDesk Server {}.", update.version))
                }
                Err(e) => {
                    record(data_dir, "Install failed", &update.version, &format!("{e:#}"));
                    tracing::warn!("server update: install failed, service left running: {e:#}");
                    Err(e)
                }
            }
        }
    }
}

fn plan_for(data_dir: &Path, settings: &Settings, available: Option<String>, manual: bool) -> Plan {
    decide(&situation(data_dir, settings, available, manual))
}

/// Called about once a minute by the running server. Does the network check
/// only when the saved interval has elapsed.
pub fn service_tick(data_dir: &Path) {
    UpdateStatus::reconcile(data_dir);
    let settings = Settings::load(data_dir);
    if !settings.auto {
        return;
    }
    let status = UpdateStatus::load(data_dir);
    let now = now_secs();
    if status.last_check_at > 0 && now.saturating_sub(status.last_check_at) < settings.interval_secs() {
        return;
    }
    match install_if_planned(data_dir, false) {
        Ok(msg) => tracing::info!("server update: {msg}"),
        Err(e) => tracing::warn!("server update: {e:#}"),
    }
}

/// Removes the scheduled task older installs registered. The service checks
/// on its own. A task that launches from svchost is what Defender was matching.
pub fn retire_legacy_task_if_disabled(_data_dir: &Path) {
    let _ = delete_task();
}

/// Stores the setting and deletes any leftover scheduled task.
pub fn set_task(data_dir: &Path, enable: bool) -> Result<()> {
    set_auto(data_dir, enable);
    delete_task()?;
    Ok(())
}

#[cfg(windows)]
fn delete_task() -> Result<()> {
    let _ = std::process::Command::new("schtasks").args(["/Delete", "/TN", UPDATE_TASK, "/F"]).status();
    Ok(())
}

#[cfg(not(windows))]
fn delete_task() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Situation {
        Situation {
            auto: true,
            manual: false,
            now: 1_000_000,
            minute_of_day: 12 * 60,
            window: None,
            current: "0.4.0".into(),
            available: Some("0.5.0".into()),
            role_active: false,
            our_epoch: 3,
            peers: Vec::new(),
            our_instance: "aaaa".into(),
            pending_since: 0,
            max_wait: MAX_WAIT_SECS,
        }
    }

    fn peer(instance: &str, version: &str, epoch: u64, at: u64) -> PeerHealth {
        PeerHealth {
            instance: instance.into(),
            name: "backup".into(),
            version: version.into(),
            role: "standby".into(),
            epoch,
            healthy: true,
            at,
        }
    }

    #[test]
    fn standby_installs_before_the_primary() {
        let standby = base();
        assert_eq!(decide(&standby), Plan::Install);
        let mut primary = base();
        primary.role_active = true;
        assert_eq!(decide(&primary), Plan::Install, "no standby is known");
        primary.peers.push(peer("bbbb", "0.4.0", 3, primary.now - 60));
        assert_eq!(decide(&primary), Plan::Defer { reason: "waiting for the standby to update" });
        primary.peers[0].version = "0.5.0".into();
        assert_eq!(decide(&primary), Plan::Install);
        primary.peers[0].version = "0.4.0".into();
        primary.pending_since = primary.now - MAX_WAIT_SECS;
        assert_eq!(decide(&primary), Plan::Install, "the wait has a limit");
    }

    #[test]
    fn unhealthy_or_stale_or_behind_epoch_does_not_count() {
        let mut primary = base();
        primary.role_active = true;
        primary.peers.push(peer("bbbb", "0.5.0", 3, primary.now - 60));
        primary.peers[0].healthy = false;
        assert_eq!(decide(&primary), Plan::Defer { reason: "waiting for the standby to update" });
        primary.peers[0].healthy = true;
        primary.peers[0].epoch = 2;
        assert_eq!(decide(&primary), Plan::Defer { reason: "waiting for the standby to update" });
        primary.peers[0].epoch = 3;
        primary.peers[0].at = primary.now - PEER_FRESH_SECS - 1;
        assert_eq!(decide(&primary), Plan::Install, "an old report is not a live standby");
    }

    #[test]
    fn unreadable_settings_are_not_rewritten_as_off() {
        let dir = std::env::temp_dir().join(format!("pjdesk-update-unreadable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        let original = r#"{"auto":true,"jitter_secs":5}"#;
        std::fs::write(&path, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0);
            std::fs::set_permissions(&path, perms.clone()).unwrap();
            let _ = Settings::load(&dir);
            perms.set_mode(0o600);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        #[cfg(not(unix))]
        {
            let _ = Settings::load(&dir);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("true"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_downgrades_prereleases_and_obeys_the_window() {
        let mut s = base();
        s.available = Some("0.3.9".into());
        assert_eq!(decide(&s), Plan::Skip { reason: "not newer" });
        // The public feed still names 0.5.2. Neither an automatic nor a manual
        // check on 0.6.0 may install it.
        s.current = "0.6.0".into();
        s.available = Some("0.5.2".into());
        s.manual = true;
        assert_eq!(decide(&s), Plan::Skip { reason: "not newer" });
        s.manual = false;
        assert_eq!(decide(&s), Plan::Skip { reason: "not newer" });
        s.current = "0.4.0".into();
        s.available = Some("0.9.0-rc1".into());
        assert_eq!(decide(&s), Plan::Skip { reason: "pre-release" });
        s.available = Some("0.5.0".into());
        s.auto = false;
        assert_eq!(decide(&s), Plan::Skip { reason: "automatic updates are off" });
        s.auto = true;
        s.manual = true;
        s.window = Some((1 * 60, 2 * 60));
        s.minute_of_day = 12 * 60;
        assert_eq!(decide(&s), Plan::Install, "a manual install does not wait for the window");
        s.manual = false;
        assert_eq!(decide(&s), Plan::Defer { reason: "outside the maintenance window" });
        s.minute_of_day = 90;
        assert_eq!(decide(&s), Plan::Install);
        // 22:00–02:00 wraps midnight.
        s.window = Some((22 * 60, 2 * 60));
        s.minute_of_day = 23 * 60;
        assert!(in_window(s.minute_of_day, s.window));
        s.minute_of_day = 60;
        assert!(in_window(s.minute_of_day, s.window));
        s.minute_of_day = 3 * 60;
        assert!(!in_window(s.minute_of_day, s.window));
        assert!(parse_hhmm("").unwrap().is_none());
        assert_eq!(parse_hhmm("02:05").unwrap(), Some(125));
        assert!(parse_hhmm("24:00").is_err());
    }

    #[test]
    fn a_finished_install_reads_as_up_to_date_without_a_check() {
        let stale = UpdateStatus {
            last_check_at: 1_700_000_000,
            last_result: "Installing 0.6.32".into(),
            available: "0.6.32".into(),
            error: String::new(),
        };
        let settled = stale.settled("0.6.32");
        assert_eq!(settled.last_result, "Up to date");
        assert!(settled.available.is_empty());
        assert!(settled.error.is_empty());
        assert_eq!(settled.last_check_at, 1_700_000_000);

        let named_only = UpdateStatus {
            last_check_at: 9,
            last_result: "Installing 0.6.32".into(),
            available: String::new(),
            error: "left from the installer".into(),
        };
        let settled = named_only.settled("0.6.33");
        assert_eq!(settled.last_result, "Up to date");
        assert!(settled.error.is_empty());

        let newer = UpdateStatus {
            last_check_at: 1,
            last_result: "Installing 0.6.34".into(),
            available: "0.6.34".into(),
            error: String::new(),
        };
        assert_eq!(newer.clone().settled("0.6.33"), newer);

        let failed = UpdateStatus {
            last_check_at: 2,
            last_result: "Check failed".into(),
            available: String::new(),
            error: "network".into(),
        };
        assert_eq!(failed.clone().settled("0.6.33"), failed);

        let dir = std::env::temp_dir().join(format!("pjdesk-update-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let current = env!("CARGO_PKG_VERSION");
        let path = dir.join("server-update-status.json");
        std::fs::write(&path, format!(r#"{{"last_check_at":1700000000,"last_result":"Installing {current}","available":"{current}","error":""}}"#)).unwrap();
        UpdateStatus::reconcile(&dir);
        let loaded = UpdateStatus::load(&dir);
        assert_eq!(loaded.last_result, "Up to date");
        assert!(loaded.available.is_empty());
        assert_eq!(loaded.last_check_at, 1_700_000_000);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Up to date"), "{text}");
        assert!(!text.contains("Installing"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
