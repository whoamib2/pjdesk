//! Server accounts: usernames, Argon2 password hashes, login tokens and each
//! account's address book.
//!
//! Everything lives under the server data folder (locked to SYSTEM and
//! Administrators when installed as a service):
//!
//! ```text
//!   accounts.json            users, password hashes and active tokens
//!   addressbook/<user>.json  one account's saved computers
//! ```
//!
//! Passwords are verified with Argon2id and never stored in the clear. A
//! successful login mints a random token the client can save (encrypted on the
//! client with DPAPI) and present next time instead of the password. The
//! server stores only a SHA-256 of that token. Address-book files sit in the
//! same admin-only folder; the passwords inside them are the account owner's,
//! on the owner's server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use pjdesk_proto::signal::{AddressEntry, Policy, Role};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many tokens one account may hold before the oldest is dropped (one per
/// signed-in device, roughly).
const MAX_TOKENS_PER_USER: usize = 20;
/// Tokens unused for this long are discarded on the next save.
const TOKEN_TTL_SECS: u64 = 60 * 60 * 24 * 90; // 90 days

#[derive(Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    users: BTreeMap<String, User>,
    /// Server-wide rules. A 0.3.x file has no policy; the defaults keep
    /// unattended access working. A 0.3.x server that rewrites the file drops
    /// this field, which is safe: it does not enforce policy anyway.
    #[serde(default)]
    policy: Policy,
}

#[derive(Clone, Serialize, Deserialize)]
struct User {
    /// PHC string, e.g. `$argon2id$v=19$m=19456,t=2,p=1$...`.
    password_hash: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    disabled: bool,
    /// Absent in files written before 0.4.0. [`User::role`] fills that in from
    /// `is_admin`, which those files do have.
    #[serde(default)]
    role: Option<Role>,
    /// SHA-256 of each active login token (`h:<hex>`) → last-used unix seconds.
    /// Older files stored the token itself; [`migrate_token_storage`] rewrites
    /// those on load. Clients still send the raw token.
    #[serde(default)]
    tokens: BTreeMap<String, u64>,
    created: u64,
}

/// A loaded, mutable accounts store backed by `accounts.json`.
pub struct Accounts {
    dir: PathBuf,
    path: PathBuf,
    data: AccountsFile,
}

/// The outcome of a successful login.
pub struct Session {
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub token: String,
}

/// What [`Accounts::prep_password_login`] copied out so the slow Argon2 check
/// can run without holding the server's global lock.
pub struct PasswordPrep {
    pub username: String,
    /// PHC hash to check, or a dummy hash when the account does not exist.
    pub hash: String,
    pub exists: bool,
    pub disabled: bool,
}

/// A row for the server's account list in the GUI/CLI.
pub struct AccountInfo {
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub role: Role,
    pub disabled: bool,
}

impl User {
    fn role(&self) -> Role {
        self.role.unwrap_or(if self.is_admin { Role::Admin } else { Role::User })
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Usernames are case-insensitive and limited to a sane character set.
pub fn normalize_username(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn valid_username(name: &str) -> bool {
    let n = name.len();
    (1..=32).contains(&n) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

impl Accounts {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("accounts.json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // The file was written by the other identity — the SYSTEM
                // service vs. the elevated management window — with an ACL this
                // process can't read. Reclaim access and try once more rather
                // than failing outright.
                reclaim_access(&path);
                match std::fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
                }
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut data = match bytes {
            Some(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?,
            None => AccountsFile::default(),
        };
        let migrated = migrate_token_storage(&mut data);
        let mut store = Self { dir: dir.to_path_buf(), path, data };
        if migrated {
            store.save()?;
        }
        Ok(store)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.data.users.is_empty()
    }

    pub fn list(&self) -> Vec<AccountInfo> {
        self.data
            .users
            .iter()
            .map(|(username, u)| AccountInfo {
                username: username.clone(),
                display_name: u.display_name.clone(),
                is_admin: u.role().is_admin(),
                role: u.role(),
                disabled: u.disabled,
            })
            .collect()
    }

    /// Creates (or, with `overwrite`, replaces the password of) an account.
    pub fn create(&mut self, username: &str, password: &str, is_admin: bool, overwrite: bool) -> Result<()> {
        let username = normalize_username(username);
        if !valid_username(&username) {
            bail!("username must be 1-32 characters: letters, digits, '.', '_' or '-'");
        }
        // One shared check for the GUI, the CLI, and any other caller.
        // Existing hashes are not rewritten; only a new or changed password
        // comes through here.
        crate::password_policy::require(password, &username)?;
        let exists = self.data.users.contains_key(&username);
        if exists && !overwrite {
            bail!("account \"{username}\" already exists");
        }
        let hash = hash_password(password)?;
        let role = if is_admin { Role::Admin } else { Role::User };
        if let Some(user) = self.data.users.get_mut(&username) {
            user.password_hash = hash;
            user.is_admin = is_admin;
            user.role = Some(role);
            user.disabled = false;
            // A password change invalidates saved tokens.
            user.tokens.clear();
        } else {
            self.data.users.insert(
                username.clone(),
                User {
                    password_hash: hash,
                    display_name: username.clone(),
                    is_admin,
                    role: Some(role),
                    disabled: false,
                    tokens: BTreeMap::new(),
                    created: now(),
                },
            );
        }
        self.save()
    }

    /// Replaces the password of an existing account and drops its saved tokens.
    pub fn set_password(&mut self, username: &str, password: &str) -> Result<()> {
        let username = normalize_username(username);
        crate::password_policy::require(password, &username)?;
        let user = self.data.users.get_mut(&username).context("no such account")?;
        user.password_hash = hash_password(password)?;
        user.tokens.clear();
        self.save()
    }

    pub fn set_role(&mut self, username: &str, role: Role) -> Result<()> {
        let username = normalize_username(username);
        let user = self.data.users.get_mut(&username).context("no such account")?;
        user.role = Some(role);
        user.is_admin = role.is_admin();
        self.save()
    }

    pub fn role_of(&self, username: &str) -> Option<Role> {
        self.data.users.get(&normalize_username(username)).map(|u| u.role())
    }

    pub fn policy(&self) -> Policy {
        self.data.policy.clone()
    }

    pub fn set_policy(&mut self, mut policy: Policy) -> Result<()> {
        policy.tunnel_ports.retain(|p| *p != 0);
        policy.tunnel_ports.sort_unstable();
        policy.tunnel_ports.dedup();
        policy.tunnel_ports.truncate(32);
        policy.max_permissions &= pjdesk_proto::perms::ALL;
        self.data.policy = policy;
        self.save()
    }

    pub fn set_disabled(&mut self, username: &str, disabled: bool) -> Result<()> {
        let username = normalize_username(username);
        let user = self.data.users.get_mut(&username).context("no such account")?;
        user.disabled = disabled;
        if disabled {
            user.tokens.clear();
        }
        self.save()
    }

    pub fn delete(&mut self, username: &str) -> Result<()> {
        let username = normalize_username(username);
        if self.data.users.remove(&username).is_none() {
            bail!("no such account");
        }
        let _ = std::fs::remove_file(self.addressbook_path(&username));
        self.save()
    }

    /// Copies the hash a password login must check. Missing accounts get a
    /// dummy hash so the check takes the same time either way. Disabled
    /// accounts still return their real hash: the caller verifies it and
    /// [`commit_password_login`] then refuses, with the same error text.
    pub fn prep_password_login(&self, username: &str) -> PasswordPrep {
        let username = normalize_username(username);
        match self.data.users.get(&username) {
            Some(u) => PasswordPrep { username, hash: u.password_hash.clone(), exists: true, disabled: u.disabled },
            None => PasswordPrep { username, hash: dummy_hash().to_string(), exists: false, disabled: false },
        }
    }

    /// Mints a token after the caller has already checked `prep.hash`. Fails
    /// closed if the account changed or is disabled since the check.
    pub fn commit_password_login(&mut self, prep: &PasswordPrep) -> Result<Session> {
        if !prep.exists || prep.disabled {
            bail!("wrong username or password");
        }
        let Some(user) = self.data.users.get(&prep.username) else {
            bail!("wrong username or password");
        };
        if user.disabled || user.password_hash != prep.hash {
            bail!("wrong username or password");
        }
        let display_name = user.display_name.clone();
        let is_admin = user.role().is_admin();
        let token = mint_token();
        let user = self.data.users.get_mut(&prep.username).unwrap();
        user.tokens.insert(token_id(&token), now());
        prune_tokens(&mut user.tokens);
        self.save()?;
        Ok(Session { username: prep.username.clone(), display_name, is_admin, token })
    }

    /// Checks a password and, on success, mints and stores a login token.
    ///
    /// The running server uses [`Self::prep_password_login`] and
    /// [`Self::commit_password_login`] so the slow check does not hold the
    /// process-wide lock. This combined form is what tests and other callers
    /// use.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn login_password(&mut self, username: &str, password: &str, _device: &str) -> Result<Session> {
        let prep = self.prep_password_login(username);
        if verify_password(&prep.hash, password).is_err() {
            bail!("wrong username or password");
        }
        self.commit_password_login(&prep)
    }

    /// Validates a saved token, refreshing its last-used time.
    ///
    /// Unknown accounts and bad tokens get the same message, so this cannot
    /// be used to learn which usernames exist.
    pub fn login_token(&mut self, username: &str, token: &str) -> Result<Session> {
        let username = normalize_username(username);
        let id = token_id(token);
        let Some(user) = self.data.users.get_mut(&username) else {
            bail!("your saved sign-in has expired; please sign in again");
        };
        if user.disabled || !user.tokens.contains_key(&id) {
            bail!("your saved sign-in has expired; please sign in again");
        }
        user.tokens.insert(id, now());
        let session = Session {
            username: username.clone(),
            display_name: user.display_name.clone(),
            is_admin: user.role().is_admin(),
            token: token.to_string(),
        };
        self.save()?;
        Ok(session)
    }

    /// Finds the account a token belongs to (for per-request authorization).
    pub fn user_for_token(&self, token: &str) -> Option<String> {
        let id = token_id(token);
        self.data.users.iter().find(|(_, u)| !u.disabled && u.tokens.contains_key(&id)).map(|(name, _)| name.clone())
    }

    pub fn logout(&mut self, token: &str) {
        let id = token_id(token);
        for user in self.data.users.values_mut() {
            if user.tokens.remove(&id).is_some() {
                let _ = self.save();
                return;
            }
        }
    }

    fn addressbook_path(&self, username: &str) -> PathBuf {
        self.dir.join("addressbook").join(format!("{username}.json"))
    }

    pub fn address_book(&self, username: &str) -> Vec<AddressEntry> {
        match std::fs::read(self.addressbook_path(username)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    pub fn save_address_book(&self, username: &str, entries: &[AddressEntry]) -> Result<()> {
        let path = self.addressbook_path(username);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
        std::fs::rename(&tmp, &path)?;
        lock_down(&path);
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        // Drop long-unused tokens whenever we persist.
        let cutoff = now().saturating_sub(TOKEN_TTL_SECS);
        for user in self.data.users.values_mut() {
            user.tokens.retain(|_, last| *last >= cutoff);
        }
        std::fs::create_dir_all(&self.dir)?;
        // Per-process temp name so a concurrent writer (GUI/CLI) can't collide.
        let tmp = self.path.with_extension(format!("json.tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        lock_down(&self.path);
        Ok(())
    }
}

/// Rewrites raw tokens left by 0.3.6 and earlier into `h:<sha256>`. Returns
/// whether anything changed. A key that already starts with `h:` is left
/// alone, so this is safe to run on every load.
fn migrate_token_storage(data: &mut AccountsFile) -> bool {
    let mut changed = false;
    for user in data.users.values_mut() {
        let old = std::mem::take(&mut user.tokens);
        for (token, seen) in old {
            if token.starts_with("h:") {
                user.tokens.insert(token, seen);
            } else {
                user.tokens.insert(token_id(&token), seen);
                changed = true;
            }
        }
    }
    changed
}

/// Storage key for a client-presented token. The raw token never hits disk.
fn token_id(token: &str) -> String {
    format!("h:{}", hex::encode(Sha256::digest(token.as_bytes())))
}

/// Windows: reclaim read access to a file the other identity created — the
/// SYSTEM service vs. the elevated management window — by taking ownership and
/// granting SYSTEM + Administrators. A no-op elsewhere, and harmless if it
/// fails: the caller still surfaces the original error if the retry can't read.
#[cfg(windows)]
fn reclaim_access(path: &Path) {
    use std::process::Stdio;
    let _ = std::process::Command::new("takeown")
        .arg("/f")
        .arg(path)
        .args(["/d", "y"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    lock_down(path);
}
#[cfg(not(windows))]
fn reclaim_access(_path: &Path) {}

/// SYSTEM and Administrators only. Removes inherited access, so a Users read
/// entry inherited from ProgramData cannot survive on a file that holds
/// password hashes or tokens. The same helper locks cloud-sync settings.
fn lock_down(path: &Path) {
    crate::cloud::protect(path);
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::encode_b64(&pjdesk_proto::random_bytes::<16>()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing failed: {e}"))?;
    Ok(hash.to_string())
}

pub(crate) fn verify_password(phc: &str, password: &str) -> Result<()> {
    let parsed = PasswordHash::new(phc).map_err(|e| anyhow::anyhow!("stored hash is invalid: {e}"))?;
    Argon2::default().verify_password(password.as_bytes(), &parsed).map_err(|_| anyhow::anyhow!("password mismatch"))
}

fn mint_token() -> String {
    hex::encode(pjdesk_proto::random_bytes::<32>())
}

/// A fixed valid hash to verify against when the username is unknown, so login
/// timing doesn't reveal whether an account exists.
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password("pjdesk-timing-equalizer").expect("dummy password hash"))
}

fn prune_tokens(tokens: &mut BTreeMap<String, u64>) {
    if tokens.len() <= MAX_TOKENS_PER_USER {
        return;
    }
    // Drop the least-recently-used ones.
    let mut by_time: Vec<(String, u64)> = tokens.iter().map(|(t, s)| (t.clone(), *s)).collect();
    by_time.sort_by_key(|(_, s)| *s);
    for (token, _) in by_time.iter().take(tokens.len() - MAX_TOKENS_PER_USER) {
        tokens.remove(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_text<T>(result: Result<T>) -> String {
        match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error"),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pjdesk-accounts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_login_and_token() {
        let dir = temp_dir("login");
        let mut a = Accounts::load(&dir).unwrap();
        assert!(a.is_empty());
        a.create("Ben", "Correct-horse-9!", true, false).unwrap();
        // Case-insensitive, wrong password rejected.
        assert!(a.login_password("ben", "nope", "pc").is_err());
        let s = a.login_password("BEN", "Correct-horse-9!", "pc").unwrap();
        assert!(s.is_admin);
        // The token round-trips through a reload, and the file stores only a hash.
        let token = s.token.clone();
        let on_disk = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
        assert!(!on_disk.contains(&token), "raw login token was written to disk");
        assert!(on_disk.contains("h:"));
        let mut a = Accounts::load(&dir).unwrap();
        assert_eq!(a.user_for_token(&token).as_deref(), Some("ben"));
        let s2 = a.login_token("ben", &token).unwrap();
        assert_eq!(s2.username, "ben");
        // A wrong token and an unknown user look the same.
        let missing = err_text(a.login_token("nobody", &token));
        let bad = err_text(a.login_token("ben", "not-a-real-token"));
        assert_eq!(missing, bad);
        // Changing the password invalidates tokens.
        a.create("ben", "Correct-horse-8!", true, true).unwrap();
        assert!(a.login_token("ben", &token).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn address_book_roundtrip() {
        let dir = temp_dir("ab");
        let mut a = Accounts::load(&dir).unwrap();
        a.create("sam", "Correct-horse-7!", false, false).unwrap();
        let entries = vec![AddressEntry {
            id: "123456789".into(),
            alias: "Office".into(),
            tags: vec!["work".into()],
            password: Some("secret".into()),
        }];
        a.save_address_book("sam", &entries).unwrap();
        assert_eq!(a.address_book("sam"), entries);
        // Deleting the account removes its address book.
        a.delete("sam").unwrap();
        assert!(a.address_book("sam").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_cannot_log_in() {
        let dir = temp_dir("disabled");
        let mut a = Accounts::load(&dir).unwrap();
        a.create("joe", "Correct-horse-6!", false, false).unwrap();
        a.set_disabled("joe", true).unwrap();
        let disabled = err_text(a.login_password("joe", "Correct-horse-6!", "pc"));
        let wrong = err_text(a.login_password("joe", "password!", "pc"));
        let missing = err_text(a.login_password("nobody", "Correct-horse-6!", "pc"));
        assert_eq!(disabled, "wrong username or password");
        assert_eq!(disabled, wrong);
        assert_eq!(disabled, missing);
        assert!(a.create("short", "short", false, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_plaintext_tokens_are_hashed_on_load() {
        let dir = temp_dir("migrate");
        let mut a = Accounts::load(&dir).unwrap();
        a.create("ben", "Correct-horse-9!", false, false).unwrap();
        // Pretend this file was written by 0.3.6, which stored the token raw.
        let path = dir.join("accounts.json");
        let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let tokens = value["users"]["ben"]["tokens"].as_object_mut().unwrap();
        tokens.clear();
        // A recent last-used time: save() drops tokens idle for 90 days, and
        // the migration saves.
        tokens.insert(
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff".into(),
            serde_json::json!(now()),
        );
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let raw = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let a = Accounts::load(&dir).unwrap();
        assert_eq!(a.user_for_token(raw).as_deref(), Some("ben"));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains(raw));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roles_survive_and_old_files_still_mean_admin() {
        let dir = temp_dir("roles");
        let mut a = Accounts::load(&dir).unwrap();
        a.create("ada", "Correct-horse-9!", true, false).unwrap();
        a.create("bea", "Correct-horse-8!", false, false).unwrap();
        assert_eq!(a.role_of("ada"), Some(Role::Admin));
        a.set_role("bea", Role::Operator).unwrap();
        assert_eq!(a.role_of("bea"), Some(Role::Operator));
        assert!(!a.list().iter().find(|i| i.username == "bea").unwrap().is_admin);
        // A 0.3.x file has is_admin and no role field.
        let legacy = r#"{"users":{"cara":{"password_hash":"x","is_admin":true,"tokens":{},"created":1}}}"#;
        std::fs::write(dir.join("accounts.json"), legacy).unwrap();
        let a = Accounts::load(&dir).unwrap();
        assert_eq!(a.role_of("cara"), Some(Role::Admin));
        let mut a = a;
        a.set_policy(Policy { force_prompt: true, tunnel_ports: vec![22, 22, 0], ..Policy::default() }).unwrap();
        let again = Accounts::load(&dir).unwrap();
        let p = again.policy();
        assert!(p.force_prompt);
        assert_eq!(p.tunnel_ports, vec![22]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
