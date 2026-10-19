//! Self-update: check a GitHub repo's latest release, download the matching
//! installer, verify its SHA-256 and (on Windows) run it silently.
//!
//! The client and server each call [`perform`] with their own installer name
//! prefix. The GitHub API and the download are plain HTTPS to github.com, so
//! this works unchanged on the user's machine (no server involvement).
//!
//! A release is installed only when its `SHA256SUMS` asset matches the
//! downloaded bytes. The installer is written under a private directory with
//! a random name, then read back and hashed again before it is launched, so
//! a local user cannot swap the file in the shared temp directory. The
//! installers are not code-signed; a signature check is deliberately not
//! required until a signing key exists (see SECURITY-AUDIT.md).

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

/// True when `s` is a pre-release (`1.2.3-rc.1`). Build metadata (`1.2.3+g1`)
/// is not a pre-release. Automatic update skips these.
pub fn is_prerelease(s: &str) -> bool {
    let s = s.trim().trim_start_matches('v');
    s.split('+').next().unwrap_or(s).contains('-')
}

/// True if `candidate` is a strictly newer release than `current`.
///
/// A pre-release candidate is never newer (those are skipped). A final
/// release is newer than a pre-release of the same numbers (`0.4.0` after
/// `0.4.0-rc1`). A smaller tuple is never newer, so a server does not
/// downgrade.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    if is_prerelease(candidate) {
        return false;
    }
    match (parse_version(current), parse_version(candidate)) {
        (Some(c), Some(n)) => n > c || (n == c && is_prerelease(current)),
        _ => false,
    }
}

/// True when `have` is a final release at least as new as `need`.
pub fn version_at_least(have: &str, need: &str) -> bool {
    if is_prerelease(have) {
        return false;
    }
    match (parse_version(have), parse_version(need)) {
        (Some(h), Some(n)) => h >= n,
        _ => false,
    }
}

fn http_get(url: &str) -> Result<Vec<u8>> {
    http_get_progress(url, &|_, _| {})
}

/// Downloads `url`. `progress` is `(bytes so far, Content-Length if the server sent one)`.
fn http_get_progress(url: &str, progress: &dyn Fn(u64, Option<u64>)) -> Result<Vec<u8>> {
    let resp = ureq::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build()
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let total = resp.header("Content-Length").and_then(|h| h.parse().ok()).filter(|n: &u64| *n > 0);
    let mut reader = resp.into_reader().take(200 * 1024 * 1024);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).with_context(|| format!("reading {url}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        progress(buf.len() as u64, total);
    }
    Ok(buf)
}

/// True for the Windows client installer, including the older `PJDesk-Setup` name
/// and a network-key rename. Never the server installer.
pub fn is_client_installer(name: &str) -> bool {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if !name.ends_with(".exe") || name.starts_with("PJDesk-Server-Setup") {
        return false;
    }
    name.starts_with("PJDesk-Client-Setup") || name.starts_with("PJDesk-Setup")
}

fn pick_android_asset(assets: &[Asset]) -> Option<&Asset> {
    assets.iter().filter(|a| is_android_apk(&a.name)).min_by_key(|a| {
        if a.name.starts_with("PJDesk-Android-") { 0u8 } else { 1 }
    })
}

/// A release APK, not a debug build and not the Windows installer.
pub fn is_android_apk(name: &str) -> bool {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".apk") && lower.contains("pjdesk") && !lower.contains("debug")
}

/// Latest Android APK from the GitHub release, when it is newer than `current`.
pub fn check_android(repo: &str, current: &str) -> Result<Option<Available>> {
    let body = http_get(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let release: Release = serde_json::from_slice(&body).context("parsing the GitHub release")?;
    if !is_newer(current, &release.tag_name) {
        return Ok(None);
    }
    let apk = pick_android_asset(&release.assets)
        .with_context(|| format!("release {} has no Android apk", release.tag_name))?;
    if !trusted_download_url(&apk.browser_download_url) {
        bail!("the release points at a download outside GitHub, so it was not offered");
    }
    Ok(Some(Available {
        version: release.tag_name.trim_start_matches('v').to_string(),
        installer_name: apk.name.clone(),
        installer_url: apk.browser_download_url.clone(),
        sums_url: None,
    }))
}

fn pick_client_asset(assets: &[Asset]) -> Option<&Asset> {
    assets
        .iter()
        .filter(|a| is_client_installer(&a.name) && !a.name.contains("pjdesk-net1:"))
        .min_by_key(|a| if a.name.starts_with("PJDesk-Client-Setup") { 0 } else { 1 })
}

/// Latest client installer. Prefers `PJDesk-Client-Setup`, then `PJDesk-Setup`.
pub fn check_client(repo: &str, current: &str) -> Result<Option<Available>> {
    let body = http_get(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let release: Release = serde_json::from_slice(&body).context("parsing the GitHub release")?;
    if !is_newer(current, &release.tag_name) {
        return Ok(None);
    }
    let installer = pick_client_asset(&release.assets)
        .with_context(|| format!("release {} has no PJDesk client installer", release.tag_name))?;
    let sums_url = release.assets.iter().find(|a| a.name == "SHA256SUMS").map(|a| a.browser_download_url.clone());
    Ok(Some(Available {
        version: release.tag_name.trim_start_matches('v').to_string(),
        installer_name: installer.name.clone(),
        installer_url: installer.browser_download_url.clone(),
        sums_url,
    }))
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

/// The SHA-256 recorded for `installer_name` in a `SHA256SUMS` file.
///
/// Lines look like `<hash>  <name>` or `<hash> *<name>` (the binary-mode marker
/// GNU coreutils writes).
pub fn expected_checksum(sums: &str, installer_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == installer_name).then(|| hash.trim().to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Release downloads must be HTTPS on a GitHub host. The checksum file is
/// published beside the installer, so a URL that leaves GitHub is not a
/// second source of truth — it is a place a tampered release listing could
/// send the updater.
fn trusted_download_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    if rest.chars().any(|c| c.is_control() || c == ' ' || c == '\\') {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') {
        return false;
    }
    let host = host.split(':').next().unwrap_or("");
    matches!(
        host,
        "github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "github-releases.githubusercontent.com"
    )
}

/// Where verified installers are written. Not the shared temp directory: the
/// update task runs as SYSTEM, and a predictable name there can be replaced
/// by another local user between the write and the launch.
/// Appends one line to the update handoff log. The installer runs as SYSTEM,
/// which can write this directory; a failure to log must not stop the restart.
pub fn record_handoff(line: &str) {
    let dir = update_dir();
    let _ = std::fs::create_dir_all(&dir);
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join("handoff.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "{secs} {line}");
    }
}

fn update_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("PJDesk").join("updates")
}

/// Creates the update directory. A symlink or junction already sitting there is
/// removed first so a local user cannot redirect the write. Files already in
/// the folder (including ones Microsoft Defender quarantined) are left in place;
/// the installer is a new random name.
///
/// `lock_acl` is only for an elevated process (the service, or `pjdesk update`
/// after the administrator prompt). The unelevated window must not call this
/// with `lock_acl` set: changing the ACL is what failed on a normal desktop.
/// A failed lockdown does not stop the update when the folder is still writable.
fn harden_update_dir(path: &std::path::Path, lock_acl: bool) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            let _ = std::fs::remove_dir(path);
            let _ = std::fs::remove_file(path);
        }
    }
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("refusing to download an update through a link at {}", path.display());
    }
    if lock_acl {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Directory only. (OI)(CI) on a file is an invalid ACE, and /T would
            // try to rewrite quarantined leftovers and fail the whole update.
            let _ = std::process::Command::new("icacls")
                .arg(path)
                .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    let probe = path.join(format!(".write-{}", std::process::id()));
    std::fs::write(&probe, b"ok").with_context(|| {
        format!(
            "could not save an update in {}. Files already there were left in place. If Microsoft Defender quarantined an older installer, restore or remove that item and try again.",
            path.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

fn safe_version(version: &str) -> String {
    let s: String = version.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').take(32).collect();
    if s.is_empty() {
        "update".into()
    } else {
        s
    }
}

/// Downloads the installer into the private update directory and checks its
/// SHA-256 against the release's `SHA256SUMS`. A release without that file is
/// refused: HTTPS to GitHub is necessary, and the checksum catches a truncated
/// or swapped download before anything is executed.
pub fn download_verified(update: &Available) -> Result<PathBuf> {
    download_verified_progress(update, &|_| {}, true)
}

fn download_verified_progress(update: &Available, progress: &dyn Fn(&str), lock_acl: bool) -> Result<PathBuf> {
    let sums_url = update.sums_url.as_deref().context(
        "this release has no SHA256SUMS file, so it was not installed. Publish SHA256SUMS next to the installer.",
    )?;
    if !trusted_download_url(&update.installer_url) || !trusted_download_url(sums_url) {
        bail!("the release points at a download outside GitHub, so it was not installed");
    }
    let version = update.version.clone();
    let last_pct = std::cell::Cell::new(999u64);
    let bytes = http_get_progress(&update.installer_url, &|got, total| {
        let pct = total.map(|total| (got.saturating_mul(100) / total).min(100)).unwrap_or(0);
        if pct / 5 != last_pct.get() / 5 {
            last_pct.set(pct);
            if total.is_some() {
                progress(&format!("Downloading PJDesk {version}… {pct}%"));
            } else {
                progress(&format!("Downloading PJDesk {version}…"));
            }
        }
    })?;
    progress("Verifying the download…");
    let sums = String::from_utf8(http_get(sums_url)?).unwrap_or_default();
    let want = expected_checksum(&sums, &update.installer_name)
        .with_context(|| format!("no checksum for {} in SHA256SUMS", update.installer_name))?;
    if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the checksum for {} is not a SHA-256 hex string", update.installer_name);
    }
    let got = sha256_hex(&bytes);
    if got != want {
        bail!("checksum mismatch for {} (expected {want}, got {got})", update.installer_name);
    }

    let dir = update_dir();
    harden_update_dir(&dir, lock_acl)?;
    let path = dir.join(format!("{}-{}.exe", hex::encode(crate::random_bytes::<8>()), safe_version(&update.version)));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_all()?;
    }
    // The directory is locked down, and the name was random, but check the
    // bytes we are about to execute anyway.
    if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
        let _ = std::fs::remove_file(&path);
        bail!("refusing to run an update that is a link");
    }
    let again = std::fs::read(&path).with_context(|| format!("reading back {}", path.display()))?;
    if sha256_hex(&again) != want {
        let _ = std::fs::remove_file(&path);
        bail!("the installer changed on disk after its checksum was verified");
    }
    Ok(path)
}

/// Runs the installer silently. The caller is expected to be elevated already
/// (the scheduled task runs as SYSTEM); the installer stops the service/app,
/// replaces the files and restarts.
#[cfg(windows)]
pub fn run_installer(path: &std::path::Path) -> Result<()> {
    std::process::Command::new(path).arg("/S").spawn().with_context(|| format!("launching {}", path.display()))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn run_installer(_path: &std::path::Path) -> Result<()> {
    bail!("installing updates is only supported on Windows")
}

#[derive(Deserialize)]
struct SiteRelease {
    version: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    sums: String,
}

/// Looks up the latest server installer. Tries `https://pjdesk.com/api/release`
/// first and, if that request fails, the GitHub latest release. A reachable
/// site that simply has nothing newer is authoritative: GitHub is not asked
/// again. Pre-releases are ignored.
pub fn check_server(current: &str) -> Result<Option<Available>> {
    match check_site(current) {
        Ok(found) => Ok(found),
        Err(_) => check(crate::UPDATE_REPO, current, "PJDesk-Server-Setup"),
    }
}

/// The version string published at `https://pjdesk.com/api/release`.
/// Callers cache this; it performs a network request.
pub fn site_version() -> Result<String> {
    let body = http_get("https://pjdesk.com/api/release")?;
    let release: SiteRelease = serde_json::from_slice(&body).context("parsing the PJDesk release")?;
    if release.version.is_empty() {
        bail!("the PJDesk release feed did not name a version");
    }
    Ok(release.version)
}

fn check_site(current: &str) -> Result<Option<Available>> {
    let body = http_get("https://pjdesk.com/api/release")?;
    let release: SiteRelease = serde_json::from_slice(&body).context("parsing the PJDesk release")?;
    if release.version.is_empty() || release.server.is_empty() || release.sums.is_empty() {
        bail!("the PJDesk release feed did not name a server installer");
    }
    if is_prerelease(&release.version) || !is_newer(current, &release.version) {
        return Ok(None);
    }
    let installer_name = release
        .server
        .rsplit('/')
        .next()
        .filter(|n| n.starts_with("PJDesk-Server-Setup") && n.ends_with(".exe"))
        .context("the release feed did not name a PJDesk Server installer")?
        .to_string();
    Ok(Some(Available {
        version: release.version.trim_start_matches('v').to_string(),
        installer_name,
        installer_url: release.server,
        sums_url: Some(release.sums),
    }))
}

/// Downloads, verifies, re-hashes the file on disk, and launches the installer.
pub fn install_available(update: &Available) -> Result<()> {
    let path = download_verified(update)?;
    // Read the bytes we are about to execute one more time. `download_verified`
    // already did this; repeating it here is the last step before the process
    // is created, so a swap in between is still refused.
    let sums_url = update.sums_url.as_deref().context(
        "this release has no SHA256SUMS file, so it was not installed. Publish SHA256SUMS next to the installer.",
    )?;
    if !trusted_download_url(sums_url) {
        bail!("the release points at a download outside GitHub, so it was not installed");
    }
    let sums = String::from_utf8(http_get(sums_url)?).unwrap_or_default();
    let want = expected_checksum(&sums, &update.installer_name)
        .with_context(|| format!("no checksum for {} in SHA256SUMS", update.installer_name))?;
    let again = std::fs::read(&path).with_context(|| format!("reading back {}", path.display()))?;
    if sha256_hex(&again) != want {
        let _ = std::fs::remove_file(&path);
        bail!("the installer changed on disk after its checksum was verified");
    }
    run_installer(&path)?;
    Ok(())
}

/// Checks, downloads, verifies, and launches the latest client installer.
///
/// `progress` is a short line for the window: checking, a download percent,
/// verifying, then the install. The installer closes PJDesk and `update-handoff`
/// opens it again, so the last line is shown before the installer starts.
/// Same as [`install_client_as_service`], but does not change folder permissions.
/// The unelevated window does not use this; it asks the service.
pub fn install_client_reporting(current: &str, progress: &dyn Fn(&str)) -> Result<Outcome> {
    install_client_reporting_lock(current, progress, false)
}

/// Check, download, verify, and install from an elevated process (the service
/// or `pjdesk update` after the administrator prompt).
pub fn install_client_as_service(current: &str, progress: &dyn Fn(&str)) -> Result<Outcome> {
    install_client_reporting_lock(current, progress, true)
}

fn install_client_reporting_lock(current: &str, progress: &dyn Fn(&str), lock_acl: bool) -> Result<Outcome> {
    progress("Checking for updates…");
    match check_client(crate::UPDATE_REPO, current)? {
        None => {
            let line = format!("PJDesk {current} is up to date.");
            progress(&line);
            Ok(Outcome::UpToDate(current.to_string()))
        }
        Some(update) => {
            let path = download_verified_progress(&update, progress, lock_acl)?;
            let line = format!("Installing PJDesk {}. PJDesk will close and open again.", update.version);
            progress(&line);
            // One beat so the window can paint that line before the installer
            // closes this process.
            std::thread::sleep(Duration::from_millis(600));
            run_installer(&path)?;
            Ok(Outcome::Installing(update.version))
        }
    }
}

/// Client self-update. Prefers `PJDesk-Client-Setup` and still accepts `PJDesk-Setup`.
pub fn perform_client(repo: &str, current: &str, check_only: bool) -> Result<Outcome> {
    match check_client(repo, current)? {
        None => Ok(Outcome::UpToDate(current.to_string())),
        Some(update) => {
            if check_only {
                return Ok(Outcome::Found(update.version));
            }
            install_available(&update)?;
            Ok(Outcome::Installing(update.version))
        }
    }
}

/// The whole flow: check, and unless `check_only`, download + verify + launch.
pub fn perform(repo: &str, current: &str, installer_prefix: &str, check_only: bool) -> Result<Outcome> {
    match check(repo, current, installer_prefix)? {
        None => Ok(Outcome::UpToDate(current.to_string())),
        Some(update) => {
            if check_only {
                return Ok(Outcome::Found(update.version));
            }
            install_available(&update)?;
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
        // Never install a pre-release, and never move to an older release.
        assert!(is_prerelease("v0.4.0-rc.1"));
        assert!(is_prerelease("0.4.0-rc1"));
        assert!(!is_prerelease("0.4.0"));
        assert!(!is_prerelease("0.4.0+gabcdef"));
        assert!(!is_newer("0.3.0", "0.4.0-rc1"));
        assert!(!is_newer("0.4.0", "0.4.0-rc1"));
        assert!(is_newer("0.4.0-rc1", "0.4.0"));
        assert!(!is_newer("0.4.1", "0.4.0"));
        // pjdesk.com/api/release still serves 0.5.2. A 0.6.0 install must not take it.
        assert!(!is_newer("0.6.0", "0.5.2"));
        assert!(!is_newer("0.6.0", "v0.5.2"));
        assert!(is_newer("0.5.2", "0.6.0"));
        assert!(version_at_least("0.4.1", "0.4.0"));
        assert!(version_at_least("0.4.0", "0.4.0"));
        assert!(!version_at_least("0.4.0-rc1", "0.4.0"));
        assert!(!version_at_least("0.3.9", "0.4.0"));
    }

    #[test]
    fn checksum_line_matches_the_installer_only() {
        let sums = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  PJDesk-Setup-0.3.7.exe
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa *PJDesk-Server-Setup-0.3.7.exe
";
        assert_eq!(
            expected_checksum(sums, "PJDesk-Setup-0.3.7.exe").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            expected_checksum(sums, "PJDesk-Server-Setup-0.3.7.exe").as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(expected_checksum(sums, "other.exe").is_none());
    }

    #[test]
    fn download_urls_stay_on_github() {
        assert!(trusted_download_url(
            "https://github.com/whoamib2/pjdesk/releases/download/v0.3.7/PJDesk-Setup-0.3.7.exe"
        ));
        assert!(trusted_download_url("https://objects.githubusercontent.com/github-production-release-asset-2e65be/a"));
        assert!(!trusted_download_url("http://github.com/whoamib2/pjdesk/releases/download/v0.3.7/a.exe"));
        assert!(!trusted_download_url("https://github.com.evil.example/a.exe"));
        assert!(!trusted_download_url("https://evil.example/github.com/a.exe"));
        assert!(!trusted_download_url("https://user:pass@github.com/a.exe"));
        assert!(!trusted_download_url("https://github.com\r\nX: y/a.exe"));
    }

    #[test]
    fn an_android_apk_is_chosen_and_a_debug_build_is_not() {
        assert!(is_android_apk("PJDesk-Android-0.6.7.apk"));
        assert!(!is_android_apk("PJDesk-Android-0.6.7-debug.apk"));
        assert!(!is_android_apk("PJDesk-Client-Setup-0.6.7.exe"));
        let assets = vec![
            Asset { name: "PJDesk-Client-Setup-0.6.7.exe".into(), browser_download_url: "exe".into() },
            Asset { name: "PJDesk-Android-0.6.7-debug.apk".into(), browser_download_url: "debug".into() },
            Asset { name: "PJDesk-Android-0.6.7.apk".into(), browser_download_url: "apk".into() },
        ];
        assert_eq!(pick_android_asset(&assets).map(|a| a.browser_download_url.as_str()), Some("apk"));
    }

    #[test]
    fn a_leftover_file_does_not_block_the_update_directory() {
        let dir = std::env::temp_dir().join(format!("pjdesk-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old-0.6.4.exe"), b"quarantined leftover").unwrap();
        let link = dir.join("linked");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("real"), &link).unwrap();
        harden_update_dir(&dir, false).unwrap();
        let dest = dir.join("new-installer.exe");
        std::fs::write(&dest, b"installer").unwrap();
        assert!(dest.is_file());
        assert_eq!(std::fs::read(dir.join("old-0.6.4.exe")).unwrap(), b"quarantined leftover");
        #[cfg(unix)]
        {
            // A link is removed and replaced with a real directory. The folder
            // it pointed at does not receive the probe file.
            harden_update_dir(&link, false).unwrap();
            assert!(link.is_dir());
            assert!(!link.symlink_metadata().unwrap().file_type().is_symlink());
            assert!(dir.join("real").read_dir().unwrap().next().is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_right_installer_name() {
        // The client prefix must not match the server asset and vice versa.
        assert!(is_client_installer("PJDesk-Setup-0.3.0.exe"));
        assert!(is_client_installer("PJDesk-Client-Setup-0.6.2.exe"));
        assert!(is_client_installer("PJDesk-Client-Setup-0.6.2--pjdesk-net1:abc.exe"));
        assert!(is_client_installer("PJDesk-Setup-0.6.2--pjdesk-net1:abc.exe"));
        assert!(!is_client_installer("PJDesk-Server-Setup-0.3.0.exe"));
        assert!(!"PJDesk-Server-Setup-0.3.0.exe".starts_with("PJDesk-Setup"));
        assert!("PJDesk-Server-Setup-0.3.0.exe".starts_with("PJDesk-Server-Setup"));
        let assets = vec![
            Asset { name: "PJDesk-Server-Setup-0.6.2.exe".into(), browser_download_url: "s".into() },
            Asset { name: "PJDesk-Setup-0.6.2.exe".into(), browser_download_url: "old".into() },
            Asset { name: "PJDesk-Client-Setup-0.6.2.exe".into(), browser_download_url: "new".into() },
        ];
        assert_eq!(pick_client_asset(&assets).map(|a| a.name.as_str()), Some("PJDesk-Client-Setup-0.6.2.exe"));
        let legacy = vec![Asset { name: "PJDesk-Setup-0.6.2.exe".into(), browser_download_url: "old".into() }];
        assert_eq!(pick_client_asset(&legacy).map(|a| a.name.as_str()), Some("PJDesk-Setup-0.6.2.exe"));
    }
}
