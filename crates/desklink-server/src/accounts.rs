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
//! client with DPAPI) and present next time instead of the password.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use desklink_proto::signal::AddressEntry;
use serde::{Deserialize, Serialize};

/// How many tokens one account may hold before the oldest is dropped (one per
/// signed-in device, roughly).
const MAX_TOKENS_PER_USER: usize = 20;
/// Tokens unused for this long are discarded on the next save.
const TOKEN_TTL_SECS: u64 = 60 * 60 * 24 * 90; // 90 days

#[derive(Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    users: BTreeMap<String, User>,
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
    /// Active login tokens → last-used unix seconds.
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

/// A row for the server's account list in the GUI/CLI.
pub struct AccountInfo {
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub disabled: bool,
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
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AccountsFile::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self { dir: dir.to_path_buf(), path, data })
    }

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
                is_admin: u.is_admin,
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
        if password.len() < 6 {
            bail!("password must be at least 6 characters");
        }
        let exists = self.data.users.contains_key(&username);
        if exists && !overwrite {
            bail!("account \"{username}\" already exists");
        }
        let hash = hash_password(password)?;
        if let Some(user) = self.data.users.get_mut(&username) {
            user.password_hash = hash;
            user.is_admin = is_admin;
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
                    disabled: false,
                    tokens: BTreeMap::new(),
                    created: now(),
                },
            );
        }
        self.save()
    }

    pub fn set_admin(&mut self, username: &str, is_admin: bool) -> Result<()> {
        let username = normalize_username(username);
        let user = self.data.users.get_mut(&username).context("no such account")?;
        user.is_admin = is_admin;
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

    /// Checks a password and, on success, mints and stores a login token.
    pub fn login_password(&mut self, username: &str, password: &str, _device: &str) -> Result<Session> {
        let username = normalize_username(username);
        let user = self.data.users.get(&username).context("wrong username or password")?;
        if user.disabled {
            bail!("this account is disabled");
        }
        verify_password(&user.password_hash, password).map_err(|_| anyhow::anyhow!("wrong username or password"))?;
        let display_name = user.display_name.clone();
        let is_admin = user.is_admin;
        let token = mint_token();
        let user = self.data.users.get_mut(&username).unwrap();
        user.tokens.insert(token.clone(), now());
        prune_tokens(&mut user.tokens);
        self.save()?;
        Ok(Session { username, display_name, is_admin, token })
    }

    /// Validates a saved token, refreshing its last-used time.
    pub fn login_token(&mut self, username: &str, token: &str) -> Result<Session> {
        let username = normalize_username(username);
        let user = self.data.users.get_mut(&username).context("unknown account")?;
        if user.disabled || !user.tokens.contains_key(token) {
            bail!("your saved sign-in has expired; please sign in again");
        }
        user.tokens.insert(token.to_string(), now());
        let session =
            Session { username: username.clone(), display_name: user.display_name.clone(), is_admin: user.is_admin, token: token.to_string() };
        self.save()?;
        Ok(session)
    }

    /// Finds the account a token belongs to (for per-request authorization).
    pub fn user_for_token(&self, token: &str) -> Option<String> {
        self.data
            .users
            .iter()
            .find(|(_, u)| !u.disabled && u.tokens.contains_key(token))
            .map(|(name, _)| name.clone())
    }

    pub fn logout(&mut self, token: &str) {
        for user in self.data.users.values_mut() {
            if user.tokens.remove(token).is_some() {
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
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        // Drop long-unused tokens whenever we persist.
        let cutoff = now().saturating_sub(TOKEN_TTL_SECS);
        for user in self.data.users.values_mut() {
            user.tokens.retain(|_, last| *last >= cutoff);
        }
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::encode_b64(&desklink_proto::random_bytes::<16>()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let hash =
        Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("hashing failed: {e}"))?;
    Ok(hash.to_string())
}

fn verify_password(phc: &str, password: &str) -> Result<()> {
    let parsed = PasswordHash::new(phc).map_err(|e| anyhow::anyhow!("stored hash is invalid: {e}"))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| anyhow::anyhow!("password mismatch"))
}

fn mint_token() -> String {
    hex::encode(desklink_proto::random_bytes::<32>())
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("desklink-accounts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_login_and_token() {
        let dir = temp_dir("login");
        let mut a = Accounts::load(&dir).unwrap();
        assert!(a.is_empty());
        a.create("Ben", "hunter2x", true, false).unwrap();
        // Case-insensitive, wrong password rejected.
        assert!(a.login_password("ben", "nope", "pc").is_err());
        let s = a.login_password("BEN", "hunter2x", "pc").unwrap();
        assert!(s.is_admin);
        // The token round-trips through a reload.
        let token = s.token.clone();
        let mut a = Accounts::load(&dir).unwrap();
        assert_eq!(a.user_for_token(&token).as_deref(), Some("ben"));
        let s2 = a.login_token("ben", &token).unwrap();
        assert_eq!(s2.username, "ben");
        // Changing the password invalidates tokens.
        a.create("ben", "newpass1", true, true).unwrap();
        assert!(a.login_token("ben", &token).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn address_book_roundtrip() {
        let dir = temp_dir("ab");
        let mut a = Accounts::load(&dir).unwrap();
        a.create("sam", "password", false, false).unwrap();
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
        a.create("joe", "password", false, false).unwrap();
        a.set_disabled("joe", true).unwrap();
        assert!(a.login_password("joe", "password", "pc").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
