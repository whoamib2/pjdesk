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
    Cleared { updated: u64 },
    Failed(String),
}

pub(super) struct LockRemote {
    sealed: String,
    updated: u64,
}

impl App {
    fn signed_in(&self) -> bool {
        self.temp.is_some() || !self.cfg.network_refresh_token().is_empty()
    }

    pub(super) fn account_email(&self) -> String {
        self.cfg.network_email.clone().unwrap_or_default().trim().to_lowercase()
    }

    pub(super) fn pin_required(&self) -> bool {
        self.signed_in() && self.cfg.app_lock_for(&self.account_email()).is_some()
    }

    fn lock_idle(&self) -> Duration {
        let secs = self.cfg.app_lock_for(&self.account_email()).map(|v| v.idle_sec).unwrap_or(pjdesk_proto::applock::IDLE_DEFAULT);
        Duration::from_secs(pjdesk_proto::applock::clamp_idle(secs))
    }

    pub(super) fn note_lock_visibility(&mut self, ctx: &egui::Context) {
        if !self.pin_required() {
            self.lock_armed = false;
            return;
        }
        if !self.lock_armed {
            self.lock_armed = true;
            self.engage_lock();
        }
        let now = Instant::now();
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if minimized {
            if self.minimized_at.is_none() {
                self.minimized_at = Some(now);
            }
            if self.minimized_at.is_some_and(|at| pjdesk_proto::applock::relock(at.elapsed().as_secs(), self.lock_idle().as_secs())) {
                self.engage_lock();
            }
        } else if let Some(at) = self.minimized_at.take() {
            if pjdesk_proto::applock::relock(at.elapsed().as_secs(), self.lock_idle().as_secs()) {
                self.engage_lock();
            }
        }
        if self.lock_ui {
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
            self.engage_lock();
        }
    }

    fn engage_lock(&mut self) {
        self.lock_ui = true;
        self.lock_pin.clear();
        self.lock_forgot = false;
        self.lock_password.clear();
        self.show_settings = false;
        self.show_invite = false;
        self.show_login = false;
    }

    pub(super) fn poll_app_lock(&mut self) {
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
        if self.lock_pull_rx.is_some() || !self.signed_in() || Instant::now() < self.next_lock_pull {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            return;
        }
        self.next_lock_pull = Instant::now() + Duration::from_secs(45);
        let (tx, rx) = mpsc::channel();
        self.lock_pull_rx = Some(rx);
        std::thread::Builder::new()
            .name("app-lock-pull".into())
            .spawn(move || {
                let result = crate::network::fetch_app_lock(&refresh, &device)
                    .map(|(sealed, updated)| LockRemote { sealed, updated })
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            })
            .ok();
    }

    fn apply_remote_lock(&mut self, remote: LockRemote) {
        if remote.updated < self.cfg.app_lock_updated {
            return;
        }
        if remote.updated == self.cfg.app_lock_updated && remote.sealed == self.cfg.app_lock_sealed {
            return;
        }
        let email = self.account_email();
        if remote.sealed.trim().is_empty() {
            // updated 0 is "no row yet", including a worker that has not been deployed.
            if remote.updated == 0 {
                return;
            }
            self.cfg.clear_app_lock();
            self.lock_ui = false;
            self.lock_armed = false;
            self.save_cfg();
            return;
        }
        let Some(key) = crate::vault::key_from(&self.cfg) else {
            self.cfg.app_lock_sealed = remote.sealed;
            self.cfg.app_lock_updated = remote.updated;
            self.save_cfg();
            return;
        };
        match pjdesk_proto::applock::open(&key, remote.sealed.trim()) {
            Ok(verifier) => {
                let changed = self.cfg.app_lock_for(&email).map(|old| old.hash != verifier.hash).unwrap_or(true);
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, remote.sealed.trim(), remote.updated, Some(&verifier));
                if changed {
                    self.cfg.clear_app_lock_fails();
                    self.engage_lock();
                    self.lock_armed = true;
                }
                self.save_cfg();
            }
            Err(err) => {
                if self.lock_ui {
                    self.lock_error = err;
                }
            }
        }
    }

    fn apply_lock_msg(&mut self, msg: LockMsg) {
        match msg {
            LockMsg::Unlocked => {
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.lock_ui = false;
                self.lock_error.clear();
                self.lock_pin.clear();
                self.last_input = Instant::now();
            }
            LockMsg::Rejected => {
                let now = super::unix_now();
                let (fails, until) = pjdesk_proto::applock::note_miss(self.cfg.app_lock_fails, now);
                self.cfg.note_app_lock_fail(fails, until);
                self.save_cfg();
                self.lock_pin.clear();
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
                self.lock_ui = false;
                self.lock_armed = true;
                self.lock_forgot = false;
                self.lock_password.clear();
                self.lock_error.clear();
                self.last_input = Instant::now();
                self.reauth_until = Instant::now() + Duration::from_secs(10 * 60);
                self.pin_note = "Signed in again. The PIN still works. Change it here if you want a new one.".into();
                self.open_settings();
                self.settings.show_security();
            }
            LockMsg::Stored { sealed, updated, verifier, key } => {
                crate::vault::set_session_key(key);
                if self.temp.is_none() {
                    self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                }
                let email = self.account_email();
                self.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
                self.cfg.write_app_lock(&email, &sealed, updated, Some(&verifier));
                self.cfg.clear_app_lock_fails();
                self.save_cfg();
                self.clear_pin_form();
                self.pin_note = "PIN saved. Every signed-in computer will ask for it.".into();
                self.lock_armed = true;
                self.next_lock_pull = Instant::now() + Duration::from_secs(45);
            }
            LockMsg::Cleared { updated } => {
                self.cfg.clear_app_lock();
                self.cfg.app_lock_updated = updated;
                self.save_cfg();
                self.lock_ui = false;
                self.lock_armed = false;
                self.clear_pin_form();
                self.pin_note = "PIN removed. This window no longer asks for one.".into();
            }
            LockMsg::Failed(err) => {
                self.lock_error = err.clone();
                self.pin_note = err;
                if let Some(v) = self.cfg.app_lock_for(&self.account_email()) {
                    self.pin_idle_minutes = (v.idle_sec / 60).clamp(1, 24 * 60) as u32;
                }
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
                ui.vertical_centered(|ui| {
                    ui.set_max_width(420.0);
                    ui.add_space(72.0);
                    ui.label(RichText::new("Enter your PIN").strong().size(22.0));
                    ui.add_space(6.0);
                    ui.label(widgets::muted(
                        ui,
                        "Your account stays signed in. This computer stays online for incoming connections and updates.",
                    ));
                    ui.add_space(16.0);
                    let mut pin = self.lock_pin.clone();
                    widgets::password_field(ui, &mut pin, "4 to 8 digits", &mut self.lock_show);
                    self.lock_pin = digits_only(&pin);
                    if !self.lock_error.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(&self.lock_error).color(p.warn));
                    }
                    ui.add_space(12.0);
                    let can = !self.lock_busy && !waiting && pjdesk_proto::applock::pin_acceptable(&self.lock_pin);
                    if widgets::accent_button(ui, if self.lock_busy { "Checking…" } else { "Unlock" }, egui::Vec2::new(420.0, 40.0)) && can {
                        self.start_unlock();
                    }
                    ui.add_space(8.0);
                    if ui.link("Forgot PIN? Sign in again").clicked() && !self.lock_busy {
                        self.lock_forgot = !self.lock_forgot;
                        self.lock_error.clear();
                    }
                    if self.lock_forgot {
                        ui.add_space(8.0);
                        ui.label(widgets::muted(ui, "This is your pjdesk.com password."));
                        let mut pass = self.lock_password.clone();
                        widgets::password_field(ui, &mut pass, "Account password", &mut self.lock_pass_show);
                        self.lock_password = pass;
                        ui.add_space(8.0);
                        if widgets::accent_button(ui, "Sign in", egui::Vec2::new(420.0, 36.0))
                            && !self.lock_busy
                            && !self.lock_password.is_empty()
                        {
                            self.start_reauth();
                        }
                    }
                });
            });
    }

    fn start_unlock(&mut self) {
        let Some(verifier) = self.cfg.app_lock_for(&self.account_email()) else {
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
                let msg = if pjdesk_proto::applock::matches(&verifier, &pin) { LockMsg::Unlocked } else { LockMsg::Rejected };
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
        let have = self.pin_required();
        ui.label(widgets::muted(
            ui,
            "A PIN for this PJDesk account. Opening this window asks for it, and so does showing it from the tray. Restoring a minimized window, or leaving it idle, asks again after the time below. The background service, incoming connections, and updates keep running.",
        ));
        ui.add_space(6.0);
        if !self.signed_in() {
            ui.label(widgets::muted(ui, "Sign in to set a PIN for this account."));
            ui.add_space(12.0);
            return;
        }
        if !have {
            ui.label("Account password");
            let mut pass = self.pin_password.clone();
            widgets::password_field(ui, &mut pass, "Account password", &mut self.pin_show_password);
            self.pin_password = pass;
        } else {
            ui.label("Current PIN");
            let mut current = self.pin_current.clone();
            widgets::password_field(ui, &mut current, "Current PIN", &mut self.pin_show_current);
            self.pin_current = digits_only(&current);
            ui.label(widgets::muted(ui, "Or your account password, if you don't have the PIN."));
            let mut pass = self.pin_password.clone();
            widgets::password_field(ui, &mut pass, "Account password", &mut self.pin_show_password);
            self.pin_password = pass;
        }
        ui.add_space(4.0);
        ui.label("New PIN");
        let mut new_pin = self.pin_new.clone();
        widgets::password_field(ui, &mut new_pin, "4 to 8 digits", &mut self.pin_show_new);
        self.pin_new = digits_only(&new_pin);
        ui.label("Confirm PIN");
        let mut confirm = self.pin_confirm.clone();
        widgets::password_field(ui, &mut confirm, "Repeat the PIN", &mut self.pin_show_confirm);
        self.pin_confirm = digits_only(&confirm);
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
            if have && ui.button("Remove PIN").clicked() && !self.lock_busy {
                self.start_save(true);
            }
        });
        if !self.pin_note.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(&self.pin_note).color(theme::paint(ui).muted).size(12.5));
        }
        ui.add_space(12.0);
    }

    fn start_save(&mut self, remove: bool) {
        let email = self.account_email();
        if email.is_empty() {
            self.pin_note = "Sign in to set a PIN.".into();
            return;
        }
        let current = self.pin_current.clone();
        let password = self.pin_password.clone();
        let new_pin = self.pin_new.clone();
        let confirm = self.pin_confirm.clone();
        let have = self.cfg.app_lock_for(&email);
        let recent = self.password_recently_ok();
        if have.is_some() && current.is_empty() && password.is_empty() && !recent {
            self.pin_note = "Enter the current PIN or your account password.".into();
            return;
        }
        if have.is_none() && password.is_empty() && !recent {
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
        if refresh.is_empty() || device.is_empty() {
            self.pin_note = "Sign in again, then set the PIN.".into();
            return;
        }
        let existing_key = crate::vault::key_from(&self.cfg);
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
        if refresh.is_empty() || device.is_empty() {
            self.pin_idle_minutes = (have.idle_sec / 60).clamp(1, 24 * 60) as u32;
            self.pin_note = "Sign in again, then choose the time.".into();
            return;
        }
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
    } else {
        return LockMsg::Failed("Enter your account password.".into());
    };
    if job.idle_only {
        let Some(mut verifier) = job.have else {
            return LockMsg::Failed("No PIN is set.".into());
        };
        verifier.idle_sec = pjdesk_proto::applock::clamp_idle(job.idle);
        verifier.updated = super::unix_now().max(1);
        let sealed = match pjdesk_proto::applock::seal(&key, &verifier) {
            Ok(s) => s,
            Err(err) => return LockMsg::Failed(err),
        };
        return match crate::network::put_app_lock(&job.refresh, &job.device, &sealed) {
            Ok(updated) => LockMsg::Stored { sealed, updated, verifier, key },
            Err(err) => LockMsg::Failed(err.to_string()),
        };
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
    match crate::network::put_app_lock(&job.refresh, &job.device, &sealed) {
        Ok(updated) => LockMsg::Stored { sealed, updated, verifier, key },
        Err(err) => LockMsg::Failed(err.to_string()),
    }
}

fn digits_only(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_digit()).take(8).collect()
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
