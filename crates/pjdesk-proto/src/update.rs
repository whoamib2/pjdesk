//! Self-update: check a GitHub repo's latest release, download the matching
//! installer, verify its SHA-256 and (on Windows) run it silently.
//!
//! The client and server each call [`perform`] with their own installer name
//! prefix. The GitHub API and the download are plain HTTPS to github.com, so
//! this works unchanged on the user's machine (no server involvement). The
//! integrity check is the HTTPS origin plus a SHA-256 from a `SHA256SUMS`
//! asset in the same release.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const USER_AGENT: &str = "PJDesk-Updater";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// An available newer release and the installer to fetch.
pub struct Available {
    pub version: String,
    pub installer_name: String,
    pub installer_url: String,
    pub sums_url: Option<String>,
}

/// What a check/update run did.
pub enum Outcome {
    /// Already on the latest version (carries the current version).
    UpToDate(String),
    /// A newer version is available (reported by `--check`).
    Found(String),
    /// Downloaded, verified and launched the installer for this version.
    Installing(String),
}

/// Parses `v1.2.3` or `1.2.3` into a comparable tuple.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True if `candidate` is a strictly newer version than `current`.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(c), Some(n)) => n > c,
        _ => false,
    }
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build()
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader().take(200 * 1024 * 1024).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Looks up the repo's latest release and the installer whose name starts with
/// `installer_prefix` (e.g. `PJDesk-Setup`). Returns `None` if not newer.
pub fn check(repo: &str, current: &str, installer_prefix: &str) -> Result<Option<Available>> {
    let body = http_get(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let release: Release = serde_json::from_slice(&body).context("parsing the GitHub release")?;
    if !is_newer(current, &release.tag_name) {
        return Ok(None);
    }
    let installer = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(installer_prefix) && a.name.ends_with(".exe"))
        .with_context(|| format!("release {} has no {installer_prefix}*.exe asset", release.tag_name))?;
    let sums_url = release.assets.iter().find(|a| a.name == "SHA256SUMS").map(|a| a.browser_download_url.clone());
    Ok(Some(Available {
        version: release.tag_name.trim_start_matches('v').to_string(),
        installer_name: installer.name.clone(),
        installer_url: installer.browser_download_url.clone(),
        sums_url,
    }))
}

/// Downloads the installer to a temp file, verifying its SHA-256 against the
/// release's `SHA256SUMS` when present.
pub fn download_verified(update: &Available) -> Result<PathBuf> {
    let bytes = http_get(&update.installer_url)?;

    if let Some(sums_url) = &update.sums_url {
        let sums = String::from_utf8(http_get(sums_url)?).unwrap_or_default();
        let want = sums
            .lines()
            .find_map(|line| {
                let (hash, name) = line.split_once(char::is_whitespace)?;
                // SHA256SUMS lines look like "<hash>  <name>" (name may have a * marker).
                let name = name.trim().trim_start_matches('*');
                (name == update.installer_name).then(|| hash.trim().to_lowercase())
            })
            .with_context(|| format!("no checksum for {} in SHA256SUMS", update.installer_name))?;
        let got = hex::encode(Sha256::digest(&bytes));
        if got != want {
            bail!("checksum mismatch for {} (expected {want}, got {got})", update.installer_name);
        }
    }

    let path = std::env::temp_dir().join(&update.installer_name);
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Runs the installer silently. The caller is expected to be elevated already
/// (the scheduled task runs as SYSTEM); the installer stops the service/app,
/// replaces the files and restarts.
#[cfg(windows)]
pub fn run_installer(path: &std::path::Path) -> Result<()> {
    std::process::Command::new(path)
        .arg("/S")
        .spawn()
        .with_context(|| format!("launching {}", path.display()))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn run_installer(_path: &std::path::Path) -> Result<()> {
    bail!("installing updates is only supported on Windows")
}

/// The whole flow: check, and unless `check_only`, download + verify + launch.
pub fn perform(repo: &str, current: &str, installer_prefix: &str, check_only: bool) -> Result<Outcome> {
    match check(repo, current, installer_prefix)? {
        None => Ok(Outcome::UpToDate(current.to_string())),
        Some(update) => {
            if check_only {
                return Ok(Outcome::Found(update.version));
            }
            let path = download_verified(&update)?;
            run_installer(&path)?;
            Ok(Outcome::Installing(update.version))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.2.0", "0.3.0"));
        assert!(is_newer("0.2.0", "v0.2.1"));
        assert!(is_newer("1.0.0", "1.0.1"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.3.0", "0.2.9"));
        assert!(!is_newer("0.2.0", "garbage"));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.2.0-rc1"), Some((0, 2, 0)));
    }

    #[test]
    fn picks_right_installer_name() {
        // The client prefix must not match the server asset and vice versa.
        assert!("PJDesk-Setup-0.3.0.exe".starts_with("PJDesk-Setup"));
        assert!(!"PJDesk-Server-Setup-0.3.0.exe".starts_with("PJDesk-Setup"));
        assert!("PJDesk-Server-Setup-0.3.0.exe".starts_with("PJDesk-Server-Setup"));
    }
}
