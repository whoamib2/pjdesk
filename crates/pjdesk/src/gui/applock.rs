//! Account PIN for the client window.
//!
//! The lock is drawn instead of the rest of the window. The host, the
//! background service, incoming connections, and update checks keep running.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

use super::{theme, widgets, App};

pub(super) enum LockMsg {
    Unlocked,
    Rejected,
    Reauthed([u8; 32]),
    Stored { sealed: String, updated: u64, verifier: pjdesk_proto::applock::Verifier, key: [u8; 32] },
    /// The PIN is on this computer. The account copy did not take it yet.
    StoredLocal { sealed: String, verifier: pjdesk_proto::applock::Verifier, key: [u8; 32], note: String },
    Cleared { updated: u64 },
    /// The password on the Set a PIN screen signed in. The PIN is saved after this when it is complete.
    SignedIn(crate::network::Session),
    Failed(String),
}

pub(super) struct LockRemote {
    sealed: String,
    updated: u64,
}

impl App {
    pub(super) fn signed_in(&self) -> bool {
        self.temp.is_some() || !self.cfg.network_refresh_token().is_empty()
    }

    pub(super) fn account_email(&self) -> String {
        self.cfg.network_email.clone().unwrap_or_default().trim().to_lowercase()
    }

    pub(super) fn pin_required(&self) -> bool {
        self.saved_pin().is_some() && (self.signed_in() || self.cfg.joined_by_code_or_key())
    }

    /// Email when this computer has one. A code or key with no email still has a local PIN subject.
    fn lock_subject(&self) -> String {
        let email = self.account_email();
        if !email.is_empty() {
            return email;
        }
        if self.cfg.joined_by_code_or_key() { "local".into() } else { String::new() }
    }

    pub(super) fn saved_pin(&self) -> Option<pjdesk_proto::applock::Verifier> {
        self.cfg.app_lock_for(&self.lock_subject())
    }

    fn lock_idle(&self) -> Duration {
        let secs = self.saved_pin().map(|v| v.idle_sec).unwrap_or(pjdesk_proto::applock::IDLE_DEFAULT);
        Duration::from_secs(pjdesk_proto::applock::clamp_idle(secs))
    }

    fn pin_screen_wanted(&self) -> bool {
        pjdesk_proto::applock::should_ask_for_pin(
            self.signed_in(),
            self.saved_pin().is_some(),
            self.cfg.joined_by_code_or_key(),
            self.cfg.pin_declined,
        )
    }

    pub(super) fn note_lock_visibility(&mut self, ctx: &egui::Context) {
        if !self.signed_in() && !self.cfg.joined_by_code_or_key() {
            self.lock_armed = false;
            self.lock_ui = false;
            return;
        }
        let has_pin = self.saved_pin().is_some();
        let ask = self.pin_screen_wanted() || self.lock_resetting;
        if !has_pin && !ask {
            self.lock_ui = false;
            self.lock_armed = true;
            return;
        }
        if !self.lock_armed {
            self.lock_armed = true;
            if has_pin || ask {
                self.engage_lock();
            }
        }
        let now = Instant::now();
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if minimized {
            if self.minimized_at.is_none() {
                self.minimized_at = Some(now);
            }
            if self
                .minimized_at
                .is_some_and(|at| pjdesk_proto::applock::relock(at.elapsed().as_secs(), self.lock_idle().as_secs()))
            {
                self.engage_lock();
            }
        } else if let Some(at) = self.minimized_at.take() {
            if pjdesk_proto::applock::relock(at.elapsed().as_secs(), self.lock_idle().as_secs()) {
                self.engage_lock();
            }
        }
        if self.lock_ui || !has_pin {
            if !has_pin && ask && !self.cfg.joined_by_code_or_key() {
                self.lock_ui = true;
            }
            return;
        }
        if user_activity(ctx) {
            self.last_input = now;
        } else if pjdesk_proto::applock::relock(self.last_input.elapsed().as_secs(), self.lock_idle().as_secs()) {
            self.engage_lock();
        }
    }

    pub(super) fn lock_from_tray(&mut self) {
        if self.pin_required() {
            tracing::info!("The window is locked. A tray open or a second launch does not skip the PIN.");
            self.engage_lock();
        }
    }

    /// Lock now (header button or tray). With a PIN set, show the PIN screen at
    /// once. Without one, open App lock so a PIN can be set.
    pub(super) fn lock_now(&mut self) {
        if self.pin_required() {
            tracing::info!("Lock now: showing the PIN screen.");
            self.engage_lock();
            self.lock_armed = true;
            return;
        }
        if !self.signed_in() && !self.cfg.joined_by_code_or_key() {
            self.toast.show("Sign in to set a PIN, then Lock now shows the PIN screen.");
            return;
        }
        tracing::info!("Lock now: no PIN is set; opening App lock to set one.");
        self.open_settings();
        self.settings.show_security();
        self.pin_note = "No PIN is set yet. Set one here, then Lock now shows the PIN screen.".into();
        self.toast.show("No PIN is set. Set one in App lock.");
    }

    fn can_lock_now(&self) -> bool {
        self.signed_in() || self.cfg.joined_by_code_or_key()
    }

    pub(super) fn lock_now_button(&mut self, ui: &mut egui::Ui) {
        if !self.can_lock_now() {
            return;
        }
        let tip = if self.pin_required() { "Show the PIN screen now (Ctrl+L)" } else { "No PIN is set. Set one to lock this window." };
        if ui.button("Lock now").on_hover_text(tip).clicked() {
            self.lock_now();
        }
    }

    fn engage_lock(&mut self) {
        self.lock_ui = true;
        self.lock_pin.clear();
        self.lock_forgot = false;
        self.lock_password.clear();
        self.lock_pin_armed = false;
        self.setup_pin_focused = false;
        self.show_settings = false;
        self.show_invite = false;
        self.show_login = false;
        self.drop_pin_peek();
    }

    fn drop_pin_peek(&mut self) {
        self.lock_peeked.clear();
        self.lock_peek_tx = None;
        self.lock_peek_rx = None;
    }

    pub(super) fn poll_app_lock(&mut self) {
        self.absorb_shared_lock();
        self.poll_pin_peek();
        if self.lock_rx.is_some() {
            let msg = self.lock_rx.as_ref().and_then(|rx| match rx.try_recv() {
                Ok(msg) => Some(msg),
                Err(mpsc::TryRecvError::Disconnected) => Some(LockMsg::Failed("Could not save the PIN.".into())),
                Err(mpsc::TryRecvError::Empty) => None,
            });
            if let Some(msg) = msg {
                self.lock_rx = None;
                self.lock_busy = false;
                self.apply_lock_msg(msg);
            }
        }
        if self.lock_pull_rx.is_some() {
            let msg = self.lock_pull_rx.as_ref().and_then(|rx| match rx.try_recv() {
                Ok(msg) => Some(msg),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Could not refresh the PIN.".into())),
                Err(mpsc::TryRecvError::Empty) => None,
            });
            if let Some(msg) = msg {
                self.lock_pull_rx = None;
                match msg {
                    Ok(remote) => self.apply_remote_lock(remote),
                    Err(err) => {
                        self.next_lock_pull = Instant::now() + Duration::from_secs(60);
                        if self.lock_ui {
                            self.lock_error = err;
                        }
                    }
                }
            }
        }
        if self.lock_pull_rx.is_some() || !self.signed_in() || self.cfg.joined_by_code_or_key() || Instant::now() < self.next_lock_pull
        {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            return;
        }
        let pending = if self.cfg.app_lock_pending { self.cfg.app_lock_sealed.clone() } else { String::new() };
        self.next_lock_pull = Instant::now() + Duration::from_secs(45);
        let (tx, rx) = mpsc::channel();
        self.lock_pull_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock-pull".into())
            .spawn(move || {
                if !pending.trim().is_empty() {
                    let _ = crate::network::put_app_lock(&refresh, &device, &pending);
                }
                let result = crate::network::fetch_app_lock(&refresh, &device)
                    .map(|(sealed, updated)| LockRemote { sealed, updated })
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            })
            .ok();
    }

    fn apply_remote_lock(&mut self, remote: LockRemote) {
        if self.cfg.joined_by_code_or_key() {
            return;
        }
        let local_verifier = self.saved_pin();
        let local = pjdesk_proto::applock::LockSide::from_verifier(
            &self.cfg.app_lock_sealed,
            self.cfg.app_lock_updated,
            local_verifier.as_ref(),
            self.cfg.app_lock_pending,
        );
        let opened = crate::vault::key_from(&self.cfg).and_then(|key| pjdesk_proto::applock::open(&key, remote.sealed.trim()).ok());
        let incoming = pjdesk_proto::applock::LockSide::from_verifier(remote.sealed.trim(), remote.updated, opened.as_ref(), false);
        match pjdesk_proto::applock::adopt_lock(&local, &incoming) {
            pjdesk_proto::applock::LockAdopt::Keep => {
                if local.generation == 0
                    && local.hash.is_empty()
                    && self.cfg.app_lock_sealed.trim().is_empty()
                    && !remote.sealed.trim().is_empty()
                {
                    self.cfg.app_lock_sealed = remote.sealed.trim().to_string();
                    self.cfg.app_lock_updated = remote.updated;
                    self.save_cfg();
                    return;
                }
                if !remote.sealed.trim().is_empty() && remote.sealed.trim() != self.cfg.app_lock_sealed.trim() {
                    let digits = local_verifier.as_ref().map(|v| v.len).unwrap_or(0);
                    let waiting = if self.cfg.app_lock_pending { ", waiting to sync" } else { "" };
                    self.note_pin_outcome(&format!(
                        "kept the PIN saved on this computer ({digits} digits{waiting}); the account copy is not newer"
                    ));
                }
            }
            pjdesk_proto::applock::LockAdopt::Stored => {
                self.cfg.app_lock_pending = false;
                if remote.updated > self.cfg.app_lock_updated {
                    self.cfg.app_lock_updated = remote.updated;
                }
                self.save_cfg();
                self.publish_shared();
                let digits = local_verifier.as_ref().map(|v| v.len).unwrap_or(0);
                self.note_pin_outcome(&format!("account copy matches the PIN on this computer ({digits} digits)"));
            }
            pjdesk_proto::applock::LockAdopt::Clear => {
                self.cfg.clear_app_lock();
                self.lock_ui = self.signed_in();
                self.lock_armed = self.signed_in();
                self.lock_resetting = false;
                self.save_cfg();
                self.publish_shared();
                self.note_pin_outcome("cleared from the account");
            }
            pjdesk_proto::applock::LockAdopt::Take => {
                let Some(verifier) = opened else {
                    return;
                };
                let email = self.account_email();
                let changed = local_verifier.as_ref().map(|old| old.hash != verifier.hash).unwrap_or(true);
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, remote.sealed.trim(), remote.updated.max(verifier.updated), Some(&verifier));
                self.cfg.app_lock_pending = false;
                if changed {
                    self.cfg.clear_app_lock_fails();
                    self.engage_lock();
                    self.lock_armed = true;
                }
                self.save_cfg();
                self.publish_shared();
                self.note_pin_outcome(&format!("took a newer account PIN ({} digits)", verifier.len));
            }
        }
    }

    fn apply_lock_msg(&mut self, msg: LockMsg) {
        match msg {
            LockMsg::Unlocked => {
                let digits = self.saved_pin().map(|v| v.len).unwrap_or(0);
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.publish_shared();
                self.note_pin_outcome(&format!("matched ({digits} digits)"));
                self.lock_ui = false;
                self.lock_resetting = false;
                self.lock_error.clear();
                self.lock_pin.clear();
                self.drop_pin_peek();
                self.last_input = Instant::now();
            }
            LockMsg::Rejected => {
                if !self.lock_ui {
                    return;
                }
                let now = super::unix_now();
                let (fails, until) = pjdesk_proto::applock::note_miss(self.cfg.app_lock_fails, now);
                self.cfg.note_app_lock_fail(fails, until);
                self.save_cfg();
                self.publish_shared();
                let digits = self.saved_pin().map(|v| v.len).unwrap_or(0);
                let changed = pjdesk_proto::applock::pin_changed_label(self.cfg.app_lock_updated, now);
                self.note_pin_outcome(&format!("rejected ({fails} misses, stored length {digits}, saved at t={})", self.cfg.app_lock_updated));
                self.lock_pin.clear();
                self.lock_peeked.clear();
                self.lock_shook = Some(Instant::now());
                self.lock_error = if until > now {
                    wait_label(until - now)
                } else {
                    let left = 5u32.saturating_sub(fails);
                    if left == 0 {
                        "Wrong PIN.".into()
                    } else {
                        format!("Wrong PIN. {left} left before a wait.")
                    }
                };
                if !changed.is_empty() {
                    self.lock_error = format!("{}\n{changed}", self.lock_error);
                }
            }
            LockMsg::Reauthed(key) => {
                crate::vault::set_session_key(key);
                if self.temp.is_none() {
                    self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                }
                if !self.cfg.app_lock_sealed.trim().is_empty() {
                    if let Ok(verifier) = pjdesk_proto::applock::open(&key, self.cfg.app_lock_sealed.trim()) {
                        let email = self.account_email();
                        let sealed = self.cfg.app_lock_sealed.clone();
                        let updated = self.cfg.app_lock_updated;
                        self.cfg.write_app_lock(&email, &sealed, updated, Some(&verifier));
                    }
                }
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.publish_shared();
                self.lock_ui = true;
                self.lock_armed = true;
                self.lock_resetting = true;
                self.setup_pin_focused = false;
                self.lock_forgot = false;
                self.lock_password.clear();
                self.lock_pin.clear();
                self.pin_new.clear();
                self.pin_confirm.clear();
                self.lock_error.clear();
                self.reauth_until = Instant::now() + Duration::from_secs(10 * 60);
                self.pin_note = "Signed in. Choose a new PIN.".into();
                tracing::info!("Account password accepted. Waiting for a new PIN.");
            }
            LockMsg::Stored { sealed, updated, verifier, key } => {
                crate::vault::set_session_key(key);
                if self.temp.is_none() {
                    self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                }
                let email = self.account_email();
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, &sealed, updated, Some(&verifier));
                self.cfg.app_lock_pending = false;
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.publish_shared();
                self.clear_pin_form();
                self.lock_resetting = false;
                self.lock_ui = false;
                self.lock_armed = true;
                self.last_input = Instant::now();
                self.pin_note = "PIN saved. This window will ask for it.".into();
                self.next_lock_pull = Instant::now() + Duration::from_secs(45);
                self.note_pin_outcome(&format!("saved and uploaded ({} digits)", verifier.len));
            }
            LockMsg::StoredLocal { sealed, verifier, key, note } => {
                crate::vault::set_session_key(key);
                if self.temp.is_none() {
                    self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                }
                let email = self.account_email();
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, &sealed, verifier.updated.max(1), Some(&verifier));
                self.cfg.app_lock_pending = true;
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.publish_shared();
                self.clear_pin_form();
                self.lock_resetting = false;
                self.lock_ui = false;
                self.lock_armed = true;
                self.last_input = Instant::now();
                self.pin_note = note.clone();
                self.toast.show(note);
                self.next_lock_pull = Instant::now() + Duration::from_secs(45);
                self.note_pin_outcome(&format!("saved on this computer ({} digits, waiting to sync)", verifier.len));
            }
            LockMsg::Cleared { updated } => {
                self.cfg.clear_app_lock();
                self.cfg.app_lock_updated = updated;
                self.save_cfg();
                self.publish_shared();
                self.lock_ui = self.signed_in();
                self.lock_armed = self.signed_in();
                self.lock_resetting = false;
                self.clear_pin_form();
                self.pin_note = "Set a new PIN to open this window.".into();
            }
            LockMsg::SignedIn(mut session) => {
                let key = session.vault_key.take();
                if let Some(key) = key {
                    crate::vault::set_session_key(key);
                    if self.temp.is_none() {
                        self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                    }
                }
                if session.email.is_empty() {
                    session.email = self.pin_email.trim().to_lowercase();
                }
                self.note_enrolled(&session.computers);
                self.cfg.prepare_sign_in_saved_sync();
                self.cfg.apply_network_session(&session);
                self.net_email = self.cfg.network_email.clone().unwrap_or_default();
                self.settings.set_server_text(&self.cfg.effective_server());
                self.save_cfg();
                crate::network::copy_to_service(&self.cfg);
                self.engine.retarget(&self.cfg);
                self.sync_passwords(true);
                self.reauth_until = Instant::now() + Duration::from_secs(10 * 60);
                self.lock_busy = false;
                let pin_ready = pjdesk_proto::applock::pin_acceptable(&self.pin_new) && self.pin_new == self.pin_confirm;
                if self.cfg.joined_by_code_or_key() || self.account_email().is_empty() {
                    self.lock_ui = false;
                    self.lock_armed = true;
                    self.pin_note.clear();
                    self.toast.show("Signed in. This computer is not asked to set a PIN.");
                } else if pin_ready {
                    self.pin_password.clear();
                    self.start_save(false);
                } else {
                    self.lock_ui = true;
                    self.lock_armed = true;
                    self.pin_note = "Signed in. Enter a PIN and choose Save PIN, or Skip for now.".into();
                }
            }
            LockMsg::Failed(err) => {
                self.note_pin_outcome(&format!("not saved ({err})"));
                self.lock_error = err.clone();
                self.pin_note = err;
                if let Some(v) = self.cfg.app_lock_for(&self.account_email()) {
                    self.pin_idle_minutes = (v.idle_sec / 60).clamp(1, 24 * 60) as u32;
                }
            }
        }
    }

    fn note_pin_outcome(&self, outcome: &str) {
        let line = format!("PIN {outcome}");
        tracing::info!("{line}");
        let path = crate::syspaths::service_dir().join("service.log");
        let _ = std::fs::create_dir_all(crate::syspaths::service_dir());
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(file, "{line} (t={})", super::unix_now());
        }
    }

    fn publish_shared(&self) {
        let path = pjdesk_proto::applock::window_lock_path();
        let disk = pjdesk_proto::applock::read_window_lock(&path);
        let disk_verifier = crate::secure::reveal(&disk.verifier)
            .and_then(|text| serde_json::from_str::<pjdesk_proto::applock::Verifier>(&text).ok());
        let local_verifier = self.saved_pin();
        let memory = pjdesk_proto::applock::LockSide::from_verifier(
            &self.cfg.app_lock_sealed,
            self.cfg.app_lock_updated,
            local_verifier.as_ref(),
            false,
        );
        let disk_side = pjdesk_proto::applock::LockSide::from_verifier(&disk.sealed, disk.updated, disk_verifier.as_ref(), false);
        let mut file = disk;
        file.email = self.account_email();
        let decision = pjdesk_proto::applock::adopt_shared_lock(&disk_side, &memory);
        let write_pin = matches!(decision, pjdesk_proto::applock::LockAdopt::Take | pjdesk_proto::applock::LockAdopt::Clear);
        if write_pin {
            file.sealed = self.cfg.app_lock_sealed.clone();
            file.updated = self.cfg.app_lock_updated;
            file.verifier = self.cfg.app_lock_verifier.clone().unwrap_or_default();
            file.fails = self.cfg.app_lock_fails;
            file.until = self.cfg.app_lock_until;
        } else if memory.hash == disk_side.hash && !memory.hash.is_empty() {
            file.fails = self.cfg.app_lock_fails;
            file.until = self.cfg.app_lock_until;
        }
        if let Err(err) = pjdesk_proto::applock::write_window_lock(&path, &file) {
            tracing::info!("Could not share the window lock: {err}");
        }
    }

    fn absorb_shared_lock(&mut self) {
        let email = self.account_email();
        if email.is_empty() {
            return;
        }
        let file = pjdesk_proto::applock::read_window_lock(&pjdesk_proto::applock::window_lock_path());
        if !file.email.eq_ignore_ascii_case(&email) {
            return;
        }
        let (fails, until) = pjdesk_proto::applock::lockout_for(&file, &email);
        if until > self.cfg.app_lock_until {
            self.cfg.note_app_lock_fail(fails, until);
            self.save_cfg();
        }
        let remote_verifier = crate::secure::reveal(&file.verifier)
            .and_then(|text| serde_json::from_str::<pjdesk_proto::applock::Verifier>(&text).ok());
        if remote_verifier.is_none() && file.sealed.trim().is_empty() {
            return;
        }
        let local_verifier = self.saved_pin();
        let local = pjdesk_proto::applock::LockSide::from_verifier(
            &self.cfg.app_lock_sealed,
            self.cfg.app_lock_updated,
            local_verifier.as_ref(),
            self.cfg.app_lock_pending,
        );
        let incoming =
            pjdesk_proto::applock::LockSide::from_verifier(file.sealed.trim(), file.updated, remote_verifier.as_ref(), false);
        match pjdesk_proto::applock::adopt_shared_lock(&local, &incoming) {
            pjdesk_proto::applock::LockAdopt::Keep | pjdesk_proto::applock::LockAdopt::Stored => {}
            pjdesk_proto::applock::LockAdopt::Clear => {
                self.cfg.clear_app_lock();
                self.lock_ui = self.signed_in();
                self.lock_armed = self.signed_in();
                self.lock_resetting = false;
                self.save_cfg();
                self.note_pin_outcome("cleared from the shared window lock");
            }
            pjdesk_proto::applock::LockAdopt::Take => {
                let Some(verifier) = remote_verifier else {
                    return;
                };
                let changed = local_verifier.as_ref().map(|old| old.hash != verifier.hash).unwrap_or(true);
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, file.sealed.trim(), file.updated.max(verifier.updated), Some(&verifier));
                self.cfg.app_lock_pending = false;
                self.save_cfg();
                if changed && self.signed_in() {
                    self.engage_lock();
                    self.lock_armed = true;
                }
                self.note_pin_outcome(&format!("took a newer PIN from the shared window lock ({} digits)", verifier.len));
            }
        }
    }

    fn clear_pin_form(&mut self) {
        self.pin_current.clear();
        self.pin_new.clear();
        self.pin_confirm.clear();
        self.pin_password.clear();
    }

    fn password_recently_ok(&self) -> bool {
        Instant::now() < self.reauth_until
    }

    /// Ask the host for a new one-time password. This does not unlock the window.
    fn refresh_one_time_password(&mut self) {
        if self.engine.service_running {
            let path = crate::syspaths::service_dir().join("password.refresh");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, b"1\n");
        } else {
            self.engine.shared.request_regenerate();
        }
        self.toast.show("New one-time password");
    }

    /// ID and one-time password for someone helping this person. Copy and
    /// refresh stay on this screen; they do not open saved computers, settings,
    /// or an outgoing connection.
    fn lock_help_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let (id, password, permanent, _) = self.share_secrets();
        let raw_id = id.as_deref().unwrap_or("");
        let raw_pw = password.as_deref().unwrap_or("");
        let help = lock_help(raw_id, raw_pw, one_time_withheld(raw_pw, permanent, false));
        theme::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Get help").strong().size(16.0).color(p.text));
                ui.add_space(8.0);
                ui.label(RichText::new("Your ID").color(p.muted).size(12.5));
                // Each line is centered as a group (value plus icons) under its label.
                let shown = if help.id.is_empty() { "— — —".to_string() } else { help.id.clone() };
                let id_icons = if help.id.is_empty() { 0 } else { 1 };
                let id_w = widgets::text_width(ui, &shown, egui::FontId::proportional(32.0)) + id_icons as f32 * (32.0 + 8.0);
                widgets::centered_row(ui, id_w, |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new(&shown).size(32.0).strong().color(p.text));
                    if !help.id.is_empty()
                        && widgets::icon_button(ui, super::icons::Icon::Copy, "Copy ID").clicked()
                        && widgets::copy_to_clipboard(&pjdesk_proto::normalize_id(&help.id))
                    {
                        self.toast.show("ID copied");
                    }
                });
                if help.password.is_none() && !help.refresh {
                    return;
                }
                ui.add_space(10.0);
                ui.label(RichText::new("One-time password").color(p.muted).size(12.5));
                let gap = 4.0;
                let value_w = match &help.password {
                    Some(pw) => widgets::text_width(ui, pw, egui::FontId::monospace(24.0)) + gap + 32.0,
                    None => 20.0 + gap + widgets::text_width(ui, "Waiting for a password", egui::FontId::proportional(14.0)),
                };
                let pw_w = value_w + if help.refresh { gap + 32.0 } else { 0.0 };
                widgets::centered_row(ui, pw_w, |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    match &help.password {
                        Some(pw) => {
                            ui.label(RichText::new(pw).size(24.0).strong().monospace().color(p.text));
                            if widgets::icon_button(ui, super::icons::Icon::Copy, "Copy password").clicked()
                                && widgets::copy_to_clipboard(pw)
                            {
                                self.toast.show("Password copied");
                            }
                        }
                        None => {
                            widgets::spinner(ui, p.muted);
                            ui.label(RichText::new("Waiting for a password").color(p.muted));
                        }
                    }
                    if help.refresh && widgets::refresh_button(ui, "New password").clicked() {
                        self.refresh_one_time_password();
                    }
                });
            });
        });
    }

    pub(super) fn lock_screen(&mut self, ui: &mut egui::Ui) {
        let now = super::unix_now();
        let until = self.cfg.app_lock_until;
        let waiting = until > now;
        if waiting {
            self.lock_error = wait_label(until - now);
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        let p = theme::paint(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::same(24)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(420.0);
                    ui.add_space(28.0);
                    self.lock_help_card(ui);
                    ui.add_space(28.0);
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(logo.id(), egui::vec2(56.0, 56.0))));
                        ui.add_space(8.0);
                    }
                    ui.label(RichText::new("PJDesk").strong().size(18.0).color(p.text));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Enter your PIN").strong().size(22.0).color(p.text));
                    ui.add_space(6.0);
                    ui.label(widgets::muted(
                        ui,
                        "Your account stays signed in. This computer stays online for incoming connections and updates.",
                    ));
                    ui.add_space(16.0);
                    let mut pin = self.lock_pin.clone();
                    let shook = self.lock_shook;
                    if shook.is_some_and(|at| at.elapsed() < Duration::from_millis(320)) {
                        ui.ctx().request_repaint_after(Duration::from_millis(16));
                    }
                    let pin_box = pin_box(ui, &mut pin, egui::Id::new("lock-pin-entry"), false, pin_shake_offset(shook), "");
                    if !self.lock_pin_armed {
                        pin_box.request_focus();
                        self.lock_pin_armed = true;
                    }
                    let prior = self.lock_pin.clone();
                    self.lock_pin = digits_only(&pin);
                    if self.lock_pin != prior && !waiting {
                        self.lock_error.clear();
                        self.consider_pin_entry();
                    }
                    let ready = !self.lock_busy && !waiting && !self.lock_pin.is_empty();
                    let can = ready && pjdesk_proto::applock::pin_acceptable(&self.lock_pin);
                    if widgets::submitted(ui, &pin_box) && ready {
                        if can {
                            self.start_unlock();
                        } else {
                            self.note_short_pin();
                        }
                    }
                    if !self.lock_error.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(&self.lock_error).color(p.warn));
                    }
                    ui.add_space(12.0);
                    if widgets::accent_button(ui, if self.lock_busy { "Checking…" } else { "Continue" }, egui::Vec2::new(420.0, 40.0)) && ready {
                        if can {
                            self.start_unlock();
                        } else {
                            self.note_short_pin();
                        }
                    }
                    ui.add_space(8.0);
                    if self.cfg.joined_by_code_or_key() {
                        if ui.link("Forgot PIN? Continue without it").clicked() && !self.lock_busy {
                            self.cfg.clear_app_lock();
                            let _ = self.cfg.save();
                            self.lock_ui = false;
                            self.lock_armed = true;
                            self.lock_error.clear();
                            self.toast.show("PIN removed. You can set one later in Settings.");
                        }
                    } else if ui.link("Forgot PIN? Sign in again").clicked() && !self.lock_busy {
                        self.lock_forgot = !self.lock_forgot;
                        self.lock_error.clear();
                    }
                    if self.lock_forgot && !self.cfg.joined_by_code_or_key() {
                        ui.add_space(8.0);
                        ui.label(widgets::muted(ui, "This is your pjdesk.com password. After it is accepted you choose a new PIN."));
                        let mut pass = self.lock_password.clone();
                        let pass_box = widgets::password_field(ui, &mut pass, "Account password", &mut self.lock_pass_show);
                        self.lock_password = pass;
                        ui.add_space(8.0);
                        let can_reset = !self.lock_busy && !self.lock_password.is_empty();
                        if widgets::submitted(ui, &pass_box) && can_reset {
                            self.start_reauth();
                        }
                        if widgets::accent_button(ui, "Sign in", egui::Vec2::new(420.0, 36.0)) && can_reset {
                            self.start_reauth();
                        }
                    }
                });
                });
            });
    }

    pub(super) fn setup_screen(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let title = if self.lock_resetting { "Choose a new PIN" } else { "Set a PIN" };
        let need_password = crate::vault::key_from(&self.cfg).is_none() && !self.password_recently_ok();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::same(24)))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(420.0);
                    ui.add_space(72.0);
                    ui.label(RichText::new(title).strong().size(22.0));
                    ui.add_space(6.0);
                    ui.label(widgets::muted(
                        ui,
                        "Set a PIN for this PJDesk account, or choose Skip for now. Incoming connections and updates keep running.",
                    ));
                    ui.add_space(16.0);
                    let local = self.cfg.joined_by_code_or_key();
                    if self.account_email().is_empty() && !local {
                        ui.label("Account email");
                        let mut email = self.pin_email.clone();
                        widgets::account_email_field(ui, &mut email);
                        self.pin_email = email;
                        ui.add_space(8.0);
                    }
                    if !local && (need_password || self.account_email().is_empty()) {
                        ui.label("Account password");
                        let mut pass = self.pin_password.clone();
                        let pass_box = widgets::password_field(ui, &mut pass, "pjdesk.com password", &mut self.pin_show_password);
                        self.pin_password = pass;
                        if widgets::submitted(ui, &pass_box) {
                            self.setup_pin_focused = false;
                        }
                        ui.add_space(8.0);
                    }
                    ui.label("PIN");
                    let mut pin = self.pin_new.clone();
                    let pin_box = pin_entry(ui, &mut pin, "setup-new", "4 to 8 digits", &mut self.pin_show_new);
                    if !self.setup_pin_focused {
                        pin_box.request_focus();
                        self.setup_pin_focused = true;
                    }
                    self.pin_new = digits_only(&pin);
                    let advance = widgets::submitted(ui, &pin_box);
                    ui.label("Confirm PIN");
                    let mut confirm = self.pin_confirm.clone();
                    let confirm_box = pin_entry(ui, &mut confirm, "confirm", "Repeat the PIN", &mut self.pin_show_confirm);
                    if advance {
                        confirm_box.request_focus();
                    }
                    self.pin_confirm = digits_only(&confirm);
                    if widgets::submitted(ui, &confirm_box) && !self.lock_busy {
                        self.start_save(false);
                    }
                    if !self.pin_note.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(&self.pin_note).color(p.warn));
                    }
                    ui.add_space(12.0);
                    let can = !self.lock_busy
                        && pjdesk_proto::applock::pin_acceptable(&self.pin_new)
                        && self.pin_new == self.pin_confirm;
                    if widgets::accent_button(ui, if self.lock_busy { "Saving…" } else { "Save PIN" }, egui::Vec2::new(420.0, 40.0)) && can {
                        self.start_save(false);
                    }
                    ui.add_space(8.0);
                    let can_sign = !self.lock_busy && !self.pin_password.is_empty();
                    if widgets::accent_button(ui, "Sign in", egui::Vec2::new(420.0, 36.0)) && can_sign {
                        if can {
                            self.start_save(false);
                        } else {
                            self.start_pin_sign_in();
                        }
                    }
                    ui.add_space(4.0);
                    if ui.button("Skip for now").clicked() && !self.lock_busy {
                        self.skip_pin_for_now();
                    }
                });
            });
    }

    /// A PIN that is not ready to check. This is not a counted miss.
    fn note_short_pin(&mut self) {
        self.lock_shook = Some(Instant::now());
        self.lock_error = "Wrong PIN.".into();
    }

    /// Hash the current PIN in the background. A match unlocks. A miss is not
    /// shown and is not counted, unless eight digits are in. Continue and Enter
    /// still use [`Self::start_unlock`].
    fn peek_pin(&mut self) {
        if self.lock_busy || self.lock_resetting || !self.lock_ui {
            return;
        }
        if self.cfg.app_lock_until > super::unix_now() {
            return;
        }
        if !pjdesk_proto::applock::pin_acceptable(&self.lock_pin) || self.lock_pin == self.lock_peeked {
            return;
        }
        let Some(verifier) = self.saved_pin() else {
            return;
        };
        let pin = self.lock_pin.clone();
        if self.lock_peek_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.lock_peek_tx = Some(tx);
            self.lock_peek_rx = Some(rx);
        }
        let Some(tx) = self.lock_peek_tx.clone() else { return };
        let spawned = std::thread::Builder::new()
            .name("app-lock-peek".into())
            .spawn(move || {
                let _ = tx.send(pjdesk_proto::applock::matches(&verifier, &pin));
            })
            .is_ok();
        if spawned {
            self.lock_peeked = self.lock_pin.clone();
        }
    }

    fn poll_pin_peek(&mut self) {
        let mut matched = false;
        let mut miss_at_max = false;
        let mut dead = false;
        if let Some(rx) = &self.lock_peek_rx {
            loop {
                match rx.try_recv() {
                    Ok(true) => matched = true,
                    Ok(false) => {
                        if self.lock_pin.chars().count() == 8 && self.lock_pin == self.lock_peeked {
                            miss_at_max = true;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
        }
        if dead {
            self.lock_peek_rx = None;
            self.lock_peek_tx = None;
        }
        if matched && self.lock_ui && !self.lock_resetting {
            self.apply_lock_msg(LockMsg::Unlocked);
            return;
        }
        if miss_at_max {
            self.start_unlock();
        }
    }

    /// After a digit. A stored length is checked only at that length. Eight
    /// digits that are not a silent check are a counted try.
    fn consider_pin_entry(&mut self) {
        if self.lock_busy || self.lock_resetting || !self.lock_ui {
            return;
        }
        if self.cfg.app_lock_until > super::unix_now() {
            return;
        }
        let n = self.lock_pin.chars().count();
        let known = self.saved_pin().map(|v| v.len).unwrap_or(0);
        if pjdesk_proto::applock::pin_silent_check(n, known) {
            self.peek_pin();
        } else if pjdesk_proto::applock::pin_miss_counts(n, false) {
            self.start_unlock();
        }
    }

    fn start_unlock(&mut self) {
        if self.lock_busy {
            return;
        }
        let Some(verifier) = self.saved_pin() else {
            self.lock_error = "No PIN is set for this account.".into();
            return;
        };
        let pin = self.lock_pin.clone();
        self.lock_busy = true;
        self.lock_error.clear();
        let (tx, rx) = mpsc::channel();
        self.lock_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let msg =
                    if pjdesk_proto::applock::matches(&verifier, &pin) { LockMsg::Unlocked } else { LockMsg::Rejected };
                let _ = tx.send(msg);
            })
            .ok();
    }

    fn start_reauth(&mut self) {
        let email = self.account_email();
        let password = self.lock_password.clone();
        self.lock_busy = true;
        self.lock_error.clear();
        let (tx, rx) = mpsc::channel();
        self.lock_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let msg = match crate::network::confirm_account_password(&email, &password) {
                    Ok(key) => LockMsg::Reauthed(key),
                    Err(err) => LockMsg::Failed(err.to_string()),
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    pub(super) fn app_lock_settings(&mut self, ui: &mut egui::Ui) {
        widgets::heading(ui, "APP LOCK");
        let local = self.cfg.joined_by_code_or_key();
        let have = self.pin_required();
        if local {
            ui.label(widgets::muted(
                ui,
                "A PIN is optional on this computer. You joined with a code or a key, so this does not ask for the account password. You can set one here.",
            ));
        } else {
            ui.label(widgets::muted(
                ui,
                "A PIN is required for this PJDesk account. Opening this window asks for it, and so does PJDesk Server, the tray, and starting the program again. Restoring a minimized window, or leaving it idle, asks again after the time below. The background service, incoming connections, and updates keep running.",
            ));
        }
        ui.add_space(6.0);
        if !self.signed_in() && !local {
            ui.label(widgets::muted(ui, "Sign in to set a PIN for this account."));
            ui.add_space(12.0);
            return;
        }
        let mut to_new = false;
        if local {
            if have {
                ui.label("Current PIN");
                let mut current = self.pin_current.clone();
                let current_box = pin_entry(ui, &mut current, "current", "Current PIN", &mut self.pin_show_current);
                self.pin_current = digits_only(&current);
                to_new = widgets::submitted(ui, &current_box);
            }
        } else if !have {
            ui.label("Account password");
            let mut pass = self.pin_password.clone();
            let pass_box = widgets::password_field(ui, &mut pass, "Account password", &mut self.pin_show_password);
            self.pin_password = pass;
            to_new = widgets::submitted(ui, &pass_box);
        } else {
            ui.label("Current PIN");
            let mut current = self.pin_current.clone();
            let current_box = pin_entry(ui, &mut current, "current", "Current PIN", &mut self.pin_show_current);
            self.pin_current = digits_only(&current);
            ui.label(widgets::muted(ui, "Or your account password, if you don't have the PIN."));
            let mut pass = self.pin_password.clone();
            let pass_box = widgets::password_field(ui, &mut pass, "Account password", &mut self.pin_show_password);
            self.pin_password = pass;
            to_new = widgets::submitted(ui, &current_box) || widgets::submitted(ui, &pass_box);
        }
        ui.add_space(4.0);
        ui.label("New PIN");
        let mut new_pin = self.pin_new.clone();
        let new_box = pin_entry(ui, &mut new_pin, "new", "4 to 8 digits", &mut self.pin_show_new);
        self.pin_new = digits_only(&new_pin);
        // Enter in the current PIN or password saves when the new PIN is ready,
        // otherwise moves on to it, like the button.
        if to_new {
            if pin_save_ready(&self.pin_new, &self.pin_confirm) && !self.lock_busy {
                self.start_save(false);
            } else {
                new_box.request_focus();
            }
        }
        let advance = widgets::submitted(ui, &new_box);
        ui.label("Confirm PIN");
        let mut confirm = self.pin_confirm.clone();
        let confirm_box = pin_entry(ui, &mut confirm, "confirm", "Repeat the PIN", &mut self.pin_show_confirm);
        if advance {
            confirm_box.request_focus();
        }
        self.pin_confirm = digits_only(&confirm);
        if widgets::submitted(ui, &confirm_box) && !self.lock_busy {
            self.start_save(false);
        }
        ui.add_space(4.0);
        ui.label("Ask again after");
        ui.horizontal(|ui| {
            for (minutes, label) in [(1, "1 min"), (5, "5 min"), (15, "15 min"), (30, "30 min")] {
                let selected = self.pin_idle_minutes == minutes;
                if ui.selectable_label(selected, label).clicked() {
                    self.pin_idle_minutes = minutes;
                    if have && !self.lock_busy {
                        self.start_idle();
                    }
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = if have { "Change PIN" } else { "Set PIN" };
            if ui.button(label).clicked() && !self.lock_busy {
                self.start_save(false);
            }
        });
        if !self.pin_note.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(&self.pin_note).color(theme::paint(ui).muted).size(12.5));
        }
        ui.add_space(12.0);
    }

    fn skip_pin_for_now(&mut self) {
        self.cfg.pin_declined = true;
        if self.cfg.pin_prompt.is_empty() {
            self.cfg.pin_prompt = pjdesk_proto::applock::PIN_PROMPT_VERSION.into();
        }
        if let Err(err) = self.cfg.save() {
            self.pin_note = format!("Could not save that choice: {err}");
            return;
        }
        self.lock_ui = false;
        self.lock_armed = true;
        self.lock_resetting = false;
        self.pin_note.clear();
        self.toast.show("PIN skipped. You can set one later in Settings.");
    }

    /// Uses the password on this screen. A session that is already signed in
    /// checks that password. Otherwise this signs in, then saves a complete PIN.
    fn start_pin_sign_in(&mut self) {
        let known = self.account_email();
        let email = if !known.is_empty() { known } else { self.pin_email.trim().to_lowercase() };
        if email.is_empty() {
            self.pin_note = "Enter the account email and password on this screen, or choose Skip for now.".into();
            return;
        }
        if self.pin_password.is_empty() {
            self.pin_note = "Enter the account password, or choose Skip for now.".into();
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        let same = !self.account_email().is_empty() && self.account_email().eq_ignore_ascii_case(&email);
        let pin_ready = pjdesk_proto::applock::pin_acceptable(&self.pin_new) && self.pin_new == self.pin_confirm;
        if !refresh.is_empty() && !device.is_empty() && same {
            if pin_ready {
                self.start_save(false);
            } else {
                self.start_reauth_from_pin();
            }
            return;
        }
        let password = self.pin_password.clone();
        let device_for_sign = if device.is_empty() { self.cfg.network_device() } else { device };
        let name = crate::hostname();
        self.lock_busy = true;
        self.pin_note = "Signing in…".into();
        let (tx, rx) = mpsc::channel();
        self.lock_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let msg = match crate::network::sign_in(&email, &password, &device_for_sign, &name) {
                    Ok(crate::network::SignIn::Ready(session)) => LockMsg::SignedIn(session),
                    Ok(crate::network::SignIn::Added { email }) => {
                        LockMsg::Failed(format!("Added this computer to {email}. That sign-in was not kept. Choose Skip for now."))
                    }
                    Ok(crate::network::SignIn::NoServer { .. }) => {
                        LockMsg::Failed(format!("{}. Choose Skip for now.", crate::network::NO_SERVER))
                    }
                    Err(err) => {
                        let text = crate::network::sign_in_pause_user_text(&err.to_string());
                        LockMsg::Failed(format!("{text}. Choose Skip for now."))
                    }
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    fn start_reauth_from_pin(&mut self) {
        let email = self.account_email();
        let password = self.pin_password.clone();
        if email.is_empty() || password.is_empty() {
            self.pin_note = "Enter the account email and password on this screen, or choose Skip for now.".into();
            return;
        }
        self.lock_busy = true;
        self.pin_note = "Signing in…".into();
        let (tx, rx) = mpsc::channel();
        self.lock_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let msg = match crate::network::confirm_account_password(&email, &password) {
                    Ok(key) => LockMsg::Reauthed(key),
                    Err(err) => {
                        let text = crate::network::sign_in_pause_user_text(&err.to_string());
                        LockMsg::Failed(format!("{text}. Choose Skip for now."))
                    }
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    fn start_save(&mut self, remove: bool) {
        if self.cfg.joined_by_code_or_key() {
            self.save_local_pin(remove);
            return;
        }
        let email = self.account_email();
        if email.is_empty() {
            let typed = self.pin_email.trim();
            if typed.is_empty() || self.pin_password.is_empty() {
                self.pin_note = "Enter the account email and password on this screen, or choose Skip for now.".into();
                return;
            }
            self.start_pin_sign_in();
            return;
        }
        let current = self.pin_current.clone();
        let password = self.pin_password.clone();
        let new_pin = self.pin_new.clone();
        let confirm = self.pin_confirm.clone();
        let have = self.cfg.app_lock_for(&email);
        let recent = self.password_recently_ok();
        let existing_key = crate::vault::key_from(&self.cfg);
        if have.is_some() && current.is_empty() && password.is_empty() && !recent {
            self.pin_note = "Enter the current PIN or your account password.".into();
            return;
        }
        if have.is_none() && password.is_empty() && !recent && existing_key.is_none() {
            self.pin_note = "Enter your account password.".into();
            return;
        }
        if !remove {
            if !pjdesk_proto::applock::pin_acceptable(&new_pin) {
                self.pin_note = "Use 4 to 8 digits.".into();
                return;
            }
            if new_pin != confirm {
                self.pin_note = "Those PINs do not match.".into();
                return;
            }
        }
        let idle = u64::from(self.pin_idle_minutes.max(1)) * 60;
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        self.spawn_save(SaveJob {
            email,
            current,
            password,
            new_pin,
            remove,
            idle_only: false,
            idle,
            refresh,
            device,
            have,
            recent,
            existing_key,
        });
    }

    fn start_idle(&mut self) {
        if self.cfg.joined_by_code_or_key() {
            self.save_local_idle();
            return;
        }
        let email = self.account_email();
        let Some(have) = self.cfg.app_lock_for(&email) else {
            return;
        };
        let idle = pjdesk_proto::applock::clamp_idle(u64::from(self.pin_idle_minutes.max(1)) * 60);
        if have.idle_sec == idle {
            return;
        }
        let password = self.pin_password.clone();
        let existing_key = crate::vault::key_from(&self.cfg);
        if existing_key.is_none() && password.is_empty() {
            self.pin_idle_minutes = (have.idle_sec / 60).clamp(1, 24 * 60) as u32;
            self.pin_note = "Enter your account password, then choose the time again.".into();
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        self.spawn_save(SaveJob {
            email,
            current: String::new(),
            password,
            new_pin: String::new(),
            remove: false,
            idle_only: true,
            idle,
            refresh,
            device,
            have: Some(have),
            recent: false,
            existing_key,
        });
    }

    /// A PIN kept on this computer. It is not sealed with the account password and it is not uploaded.
    fn save_local_pin(&mut self, remove: bool) {
        let subject = self.lock_subject();
        if subject.is_empty() {
            self.pin_note = "This computer is not connected.".into();
            return;
        }
        if remove {
            self.cfg.clear_app_lock();
            self.save_cfg();
            self.publish_shared();
            self.clear_pin_form();
            self.lock_ui = false;
            self.lock_armed = true;
            self.pin_note = "PIN removed.".into();
            return;
        }
        if let Some(have) = self.saved_pin() {
            if !pjdesk_proto::applock::matches(&have, &self.pin_current) {
                self.pin_note = "That PIN is wrong.".into();
                return;
            }
        }
        if !pjdesk_proto::applock::pin_acceptable(&self.pin_new) {
            self.pin_note = "Use 4 to 8 digits.".into();
            return;
        }
        if self.pin_new != self.pin_confirm {
            self.pin_note = "Those PINs do not match.".into();
            return;
        }
        let idle = pjdesk_proto::applock::clamp_idle(u64::from(self.pin_idle_minutes.max(1)) * 60);
        let verifier = match pjdesk_proto::applock::create(&self.pin_new, idle, super::unix_now().max(1)) {
            Ok(v) => v,
            Err(err) => {
                self.pin_note = err;
                return;
            }
        };
        self.cfg.write_app_lock(&subject, "", super::unix_now().max(1), Some(&verifier));
        self.cfg.clear_app_lock_fails();
        self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
        self.save_cfg();
        self.publish_shared();
        self.clear_pin_form();
        self.lock_ui = false;
        self.lock_armed = true;
        self.lock_resetting = false;
        self.pin_note = "PIN saved for this computer.".into();
        self.toast.show("PIN saved. This window will ask for it.");
    }

    fn save_local_idle(&mut self) {
        let Some(mut verifier) = self.saved_pin() else { return };
        let idle = pjdesk_proto::applock::clamp_idle(u64::from(self.pin_idle_minutes.max(1)) * 60);
        if verifier.idle_sec == idle {
            return;
        }
        verifier.idle_sec = idle;
        verifier.updated = super::unix_now().max(1);
        let subject = self.lock_subject();
        self.cfg.write_app_lock(&subject, "", verifier.updated, Some(&verifier));
        self.save_cfg();
        self.publish_shared();
        self.pin_note = "Saved.".into();
    }

    fn spawn_save(&mut self, job: SaveJob) {
        self.lock_busy = true;
        self.pin_note = "Saving…".into();
        let (tx, rx) = mpsc::channel();
        self.lock_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let _ = tx.send(save_lock(job));
            })
            .ok();
    }
}

struct SaveJob {
    email: String,
    current: String,
    password: String,
    new_pin: String,
    remove: bool,
    idle_only: bool,
    idle: u64,
    refresh: String,
    device: String,
    have: Option<pjdesk_proto::applock::Verifier>,
    recent: bool,
    existing_key: Option<[u8; 32]>,
}

fn save_lock(job: SaveJob) -> LockMsg {
    let key = if !job.password.is_empty() {
        match crate::network::confirm_account_password(&job.email, &job.password) {
            Ok(key) => key,
            Err(err) => return LockMsg::Failed(err.to_string()),
        }
    } else if job.idle_only {
        match job.existing_key {
            Some(key) => key,
            None => return LockMsg::Failed("Enter your account password to change how soon this asks again.".into()),
        }
    } else if job.recent {
        match job.existing_key {
            Some(key) => key,
            None => return LockMsg::Failed("Enter your account password.".into()),
        }
    } else if let Some(verifier) = &job.have {
        if !pjdesk_proto::applock::matches(verifier, &job.current) {
            return LockMsg::Failed("That PIN is wrong.".into());
        }
        match job.existing_key {
            Some(key) => key,
            None => return LockMsg::Failed("Enter your account password to save this on the account.".into()),
        }
    } else if let Some(key) = job.existing_key {
        key
    } else {
        return LockMsg::Failed("Enter your account password.".into());
    };
    if job.idle_only {
        let Some(mut verifier) = job.have.clone() else {
            return LockMsg::Failed("No PIN is set.".into());
        };
        verifier.idle_sec = pjdesk_proto::applock::clamp_idle(job.idle);
        verifier.updated = super::unix_now().max(1);
        let sealed = match pjdesk_proto::applock::seal(&key, &verifier) {
            Ok(s) => s,
            Err(err) => return LockMsg::Failed(err),
        };
        return finish_lock_upload(&job, sealed, verifier, key);
    }
    if job.remove {
        return match crate::network::put_app_lock(&job.refresh, &job.device, "") {
            Ok(updated) => LockMsg::Cleared { updated },
            Err(err) => LockMsg::Failed(err.to_string()),
        };
    }
    let verifier = match pjdesk_proto::applock::create(&job.new_pin, job.idle, super::unix_now().max(1)) {
        Ok(v) => v,
        Err(err) => return LockMsg::Failed(err),
    };
    let sealed = match pjdesk_proto::applock::seal(&key, &verifier) {
        Ok(s) => s,
        Err(err) => return LockMsg::Failed(err),
    };
    finish_lock_upload(&job, sealed, verifier, key)
}

fn finish_lock_upload(job: &SaveJob, sealed: String, verifier: pjdesk_proto::applock::Verifier, key: [u8; 32]) -> LockMsg {
    if job.refresh.is_empty() || job.device.is_empty() {
        return LockMsg::StoredLocal { sealed, verifier, key, note: local_pin_note("") };
    }
    match crate::network::put_app_lock(&job.refresh, &job.device, &sealed) {
        Ok(updated) => LockMsg::Stored { sealed, updated, verifier, key },
        Err(err) => LockMsg::StoredLocal { sealed, verifier, key, note: local_pin_note(&err.to_string()) },
    }
}

fn local_pin_note(err: &str) -> String {
    let saved = "PIN saved on this computer. It will sync to the account when the server answers.";
    if pjdesk_proto::signal::is_sign_in_required(err) || err.contains("requires sign-in") {
        format!("{err}\n{saved}")
    } else {
        saved.to_string()
    }
}

fn digits_only(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_digit()).take(8).collect()
}

/// Horizontal centers for the masked PIN dots, around 0. An empty PIN has no
/// dots, so the screen does not reserve slots for a length.
fn pin_mark_xs(count: usize, gap: f32) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let start = -((count as f32 - 1.0) * gap) / 2.0;
    (0..count).map(|i| start + i as f32 * gap).collect()
}

/// A short side-to-side move after a wrong PIN. Zero once it has finished.
fn pin_shake_offset(started: Option<Instant>) -> f32 {
    let Some(started) = started else {
        return 0.0;
    };
    let t = started.elapsed().as_secs_f32();
    if t >= 0.32 {
        return 0.0;
    }
    let damp = 1.0 - t / 0.32;
    (t * 48.0).sin() * 7.0 * damp
}

/// Keyboard entry for the lock screen PIN, as the tests drive it.
#[cfg(test)]
fn pin_key_field(ui: &mut egui::Ui, pin: &mut String) -> egui::Response {
    pin_box(ui, pin, egui::Id::new("lock-pin-entry"), false, 0.0, "")
}

/// What a key does to a PIN entry before the text field sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinKey {
    /// Remove the last digit typed.
    Back,
    /// Clear the whole entry.
    Clear,
}

fn apply_pin_key(pin: &mut String, key: PinKey) {
    match key {
        PinKey::Back => {
            pin.pop();
        }
        PinKey::Clear => pin.clear(),
    }
}

/// A visible rounded PIN box. The dots (or digits, when shown) grow out from
/// the middle; there are no slots or placeholder digits, so it never hints at
/// the length. Backspace removes the last digit typed, Delete or Esc clears it.
fn pin_box(ui: &mut egui::Ui, pin: &mut String, id: egui::Id, shown: bool, shake: f32, hint: &str) -> egui::Response {
    let p = theme::paint(ui);
    // egui drops focus on Esc before widgets run, so remember last frame's focus.
    let had = ui.data(|d| d.get_temp::<bool>(id.with("focus"))).unwrap_or(false);
    let escaped = had && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let focused = ui.memory(|m| m.has_focus(id)) || escaped;
    if focused {
        let keys = ui.input_mut(|i| {
            let mut keys = Vec::new();
            i.events.retain(|event| match event {
                egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                    keys.push(PinKey::Back);
                    false
                }
                egui::Event::Key { key: egui::Key::Delete | egui::Key::Escape, pressed: true, .. } => {
                    keys.push(PinKey::Clear);
                    false
                }
                egui::Event::Key { key: egui::Key::Backspace | egui::Key::Delete | egui::Key::Escape, .. } => false,
                _ => true,
            });
            keys
        });
        for key in keys {
            apply_pin_key(pin, key);
        }
    }
    let width = ui.available_width().min(420.0).max(48.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::hover());
    let box_rect = rect.translate(egui::vec2(shake, 0.0));
    let stroke = if focused { egui::Stroke::new(2.0, p.accent) } else { egui::Stroke::new(1.5, p.line) };
    ui.painter().rect(box_rect, egui::CornerRadius::same(12), p.elev, stroke, egui::StrokeKind::Inside);
    // Lay out the real text, only invisible. An empty layout pins the cursor at
    // 0, so every digit went in front of the last one and "1234" was checked
    // as "4321" (0.6.50 to 0.6.65).
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
        ui.fonts_mut(|fonts| fonts.layout(text.as_str().to_owned(), egui::FontId::proportional(14.0), egui::Color32::TRANSPARENT, wrap))
    };
    let saved = ui.visuals().text_cursor.stroke;
    ui.visuals_mut().text_cursor.stroke = egui::Stroke::NONE;
    let resp = ui.put(
        rect.shrink2(egui::vec2(12.0, 12.0)),
        egui::TextEdit::singleline(pin)
            .id(id)
            .password(true)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(rect.width() - 24.0)
            .char_limit(8)
            .layouter(&mut layouter),
    );
    ui.visuals_mut().text_cursor.stroke = saved;
    if escaped {
        resp.request_focus();
    }
    let keep = resp.has_focus() || escaped;
    ui.data_mut(|d| d.insert_temp(id.with("focus"), keep));
    let count = pin.chars().count();
    let center = box_rect.center();
    if count == 0 {
        if !hint.is_empty() {
            ui.painter().text(center, egui::Align2::CENTER_CENTER, hint, egui::FontId::proportional(14.0), p.muted);
        }
    } else if shown {
        ui.painter().text(center, egui::Align2::CENTER_CENTER, pin.as_str(), egui::FontId::monospace(18.0), p.text);
    } else {
        for (i, x) in pin_mark_xs(count, 18.0).into_iter().enumerate() {
            let appear = ui.ctx().animate_bool_with_time(id.with(("dot", i)), true, 0.16);
            let radius = 5.0 * (0.7 + 0.3 * appear);
            ui.painter().circle_filled(center + egui::vec2(x, 0.0), radius, p.text.linear_multiply(0.35 + 0.65 * appear));
        }
    }
    for i in count..8 {
        ui.ctx().animate_bool_with_time(id.with(("dot", i)), false, 0.01);
    }
    resp
}

/// A PIN box with a Show button, for the set and confirm screens.
fn pin_entry(ui: &mut egui::Ui, pin: &mut String, key: &str, hint: &str, shown: &mut bool) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let show_w = 64.0;
        let resp = ui
            .allocate_ui(egui::vec2((ui.available_width() - show_w - 8.0).max(48.0), 48.0), |ui| {
                pin_box(ui, pin, egui::Id::new(("pin-entry", key)), *shown, 0.0, hint)
            })
            .inner;
        let label = if *shown { "Hide" } else { "Show" };
        if ui.add_sized([show_w, 36.0], egui::Button::new(label)).on_hover_text(label).clicked() {
            *shown = !*shown;
        }
        resp
    })
    .inner
}

fn user_activity(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        i.pointer.any_pressed()
            || i.pointer.any_released()
            || i.smooth_scroll_delta.length_sq() > 0.0
            || i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::PointerMoved(_)
                )
            })
    })
}

/// What the PIN screen may show so someone can get help without unlocking.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LockHelp {
    /// This computer's ID, spaced for reading. Empty when it has none yet.
    id: String,
    /// The current one-time password, when it is the password to read out.
    password: Option<String>,
    /// A new one-time password can be requested. Off when that password is withheld.
    refresh: bool,
}

/// `withheld` is true when the one-time password is permanent-only or hidden.
/// The screen then keeps the ID and leaves the password off.
fn lock_help(id: &str, one_time: &str, withheld: bool) -> LockHelp {
    let digits = pjdesk_proto::normalize_id(id);
    let id = if pjdesk_proto::is_valid_id(&digits) {
        pjdesk_proto::format_id(&digits)
    } else {
        id.trim().to_string()
    };
    let password = if withheld {
        None
    } else {
        let pw = one_time.trim();
        if pw.is_empty() {
            None
        } else {
            Some(pw.to_string())
        }
    };
    LockHelp { id, password, refresh: !withheld }
}

/// The one-time password stays off the lock screen when it is hidden, or when
/// a permanent password is set and no one-time password is published.
/// A permanent password that sits beside a shown one-time password does not
/// hide that one-time password.
fn one_time_withheld(one_time: &str, permanent_set: bool, hidden: bool) -> bool {
    hidden || (one_time.trim().is_empty() && permanent_set)
}

fn wait_label(secs: u64) -> String {
    if secs >= 60 {
        let minutes = secs.div_ceil(60);
        if minutes == 1 {
            "Too many tries. Try again in 1 minute.".into()
        } else {
            format!("Too many tries. Try again in {minutes} minutes.")
        }
    } else {
        format!("Too many tries. Try again in {secs} seconds.")
    }
}

/// Whether the new PIN and its confirmation are ready to save.
pub(crate) fn pin_save_ready(new_pin: &str, confirm: &str) -> bool {
    pjdesk_proto::applock::pin_acceptable(new_pin) && new_pin == confirm
}

#[cfg(test)]
mod tests {
    #[test]
    fn enter_saves_only_a_complete_matching_pin() {
        assert!(super::pin_save_ready("4821", "4821"));
        assert!(!super::pin_save_ready("4821", "4822"));
        assert!(!super::pin_save_ready("48", "48"));
        assert!(!super::pin_save_ready("", ""));
    }

    use std::time::{Duration, Instant};

    use super::{lock_help, one_time_withheld, pin_mark_xs, pin_shake_offset};

    #[test]
    fn the_lock_screen_shows_the_id_and_one_time_password_unless_it_is_hidden() {
        let shown = lock_help("386626882", "k7mq2w9p", false);
        assert_eq!(shown.id, "386 626 882");
        assert_eq!(shown.password.as_deref(), Some("k7mq2w9p"));
        assert!(shown.refresh);

        assert!(
            one_time_withheld("", true, false),
            "a permanent password with no one-time password is permanent-only"
        );
        let permanent_only = lock_help("386 626 882", "k7mq2w9p", true);
        assert_eq!(permanent_only.id, "386 626 882");
        assert_eq!(permanent_only.password, None);
        assert!(!permanent_only.refresh, "refresh stays off when the one-time password is withheld");

        assert!(one_time_withheld("k7mq2w9p", false, true), "a hidden one-time password is withheld");
        assert!(
            !one_time_withheld("k7mq2w9p", true, false),
            "a published one-time password is still shown when a permanent password is also set"
        );

        let waiting = lock_help("386626882", "  ", false);
        assert_eq!(waiting.password, None);
        assert!(waiting.refresh, "a password that has not arrived yet can still be refreshed");
        assert!(
            !one_time_withheld("", false, false),
            "no password yet is not permanent-only until a permanent password is set"
        );
    }

    #[test]
    fn pin_marks_grow_from_the_center_and_do_not_reserve_a_length() {
        assert!(pin_mark_xs(0, 18.0).is_empty(), "an empty PIN is a line, not a row of slots");
        assert_eq!(pin_mark_xs(1, 18.0), vec![0.0]);
        assert_eq!(pin_mark_xs(2, 18.0), vec![-9.0, 9.0]);
        assert_eq!(pin_mark_xs(3, 18.0), vec![-18.0, 0.0, 18.0]);
        let five = pin_mark_xs(5, 16.0);
        assert_eq!(five.len(), 5);
        assert!((five[0] + five[4]).abs() < 0.01, "dots stay centered as they are added");
        assert_eq!(pin_shake_offset(None), 0.0);
        assert_eq!(pin_shake_offset(Some(Instant::now() - Duration::from_secs(2))), 0.0);
    }

    /// Type into a field the way egui delivers keys: one Text event a frame.
    fn type_into(field: fn(&mut eframe::egui::Ui, &mut String) -> eframe::egui::Response, digits: &str) -> String {
        use eframe::egui;
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut step = |event: Option<egui::Event>, text: &mut String| {
            let mut input = egui::RawInput::default();
            input.events.extend(event);
            let mut out = ctx.run_ui(input, |ui| field(ui, text).request_focus());
            out.textures_delta.clear();
        };
        step(None, &mut text);
        step(None, &mut text);
        for c in digits.chars() {
            step(Some(egui::Event::Text(c.to_string())), &mut text);
        }
        text
    }

    fn setup_field(ui: &mut eframe::egui::Ui, text: &mut String) -> eframe::egui::Response {
        let mut shown = false;
        super::pin_entry(ui, text, "setup-new", "4 to 8 digits", &mut shown)
    }

    fn key(key: eframe::egui::Key) -> eframe::egui::Event {
        eframe::egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    /// Type a script: digits are typed, '<' is Backspace, 'x' is Delete, 'e' is Esc.
    fn script(field: fn(&mut eframe::egui::Ui, &mut String) -> eframe::egui::Response, keys: &str) -> String {
        use eframe::egui;
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut step = |event: Option<egui::Event>, text: &mut String| {
            let mut input = egui::RawInput::default();
            input.events.extend(event);
            let mut out = ctx.run_ui(input, |ui| field(ui, text).request_focus());
            out.textures_delta.clear();
        };
        step(None, &mut text);
        step(None, &mut text);
        for c in keys.chars() {
            let event = match c {
                '<' => key(egui::Key::Backspace),
                'x' => key(egui::Key::Delete),
                'e' => key(egui::Key::Escape),
                c => egui::Event::Text(c.to_string()),
            };
            step(Some(event), &mut text);
        }
        text
    }

    #[test]
    fn backspace_removes_the_last_digit_typed_and_delete_or_esc_clears() {
        for field in [super::pin_key_field as fn(&mut eframe::egui::Ui, &mut String) -> eframe::egui::Response, setup_field] {
            assert_eq!(script(field, "1234<"), "123", "Backspace removes the last digit typed");
            assert_eq!(script(field, "1234<<5"), "125");
            assert_eq!(script(field, "<"), "", "Backspace on an empty box does nothing");
            assert_eq!(script(field, "1234x"), "", "Delete clears");
            assert_eq!(script(field, "1234e"), "", "Esc clears");
            assert_eq!(script(field, "12e34"), "34", "the box keeps focus after Esc");
        }
    }

    #[test]
    fn the_lock_field_keeps_digits_in_the_order_they_were_typed() {
        assert_eq!(type_into(super::pin_key_field, "1234"), "1234");
        assert_eq!(type_into(super::pin_key_field, "90817265"), "90817265");
        assert_eq!(type_into(super::pin_key_field, "123456789"), "12345678", "eight digits at most");
    }

    #[test]
    fn a_pin_set_on_the_setup_screen_unlocks_on_the_lock_screen() {
        use pjdesk_proto::applock::{create, matches};
        // Set, then verify.
        let set = super::digits_only(&type_into(setup_field, "2580"));
        let first = create(&set, 300, 1).unwrap();
        let typed = super::digits_only(&type_into(super::pin_key_field, "2580"));
        assert!(matches(&first, &typed), "the same PIN typed on the lock screen unlocks");
        assert!(!matches(&first, "0852"));
        // Reset (Forgot PIN, then a new PIN), then verify.
        let reset = super::digits_only(&type_into(setup_field, "731946"));
        let second = create(&reset, 300, 2).unwrap();
        assert!(matches(&second, &super::digits_only(&type_into(super::pin_key_field, "731946"))));
        assert!(!matches(&second, &typed), "the old PIN no longer unlocks after a reset");
    }
}
