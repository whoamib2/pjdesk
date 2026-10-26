//! Account PIN for the PJDesk Server window.
//!
//! The service, incoming connections, and update checks keep running. This
//! module only decides what the window is allowed to draw.

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText};

use pjdesk_proto::applock::{self, Verifier, WindowGate, WindowLockFile};

enum Msg {
    Unlocked,
    Rejected,
    Reauthed([u8; 32]),
    Stored { sealed: String, updated: u64, verifier: Verifier, key: [u8; 32] },
    Failed(String),
    Remote(Result<Remote, String>),
}

struct Remote {
    sealed: String,
    updated: u64,
}

pub struct Gate {
    armed: bool,
    ui: bool,
    resetting: bool,
    email: String,
    token: String,
    api: String,
    pin: String,
    pin_show: bool,
    confirm: String,
    confirm_show: bool,
    password: String,
    pass_show: bool,
    forgot: bool,
    error: String,
    note: String,
    busy: bool,
    verifier: Option<Verifier>,
    sealed: String,
    updated: u64,
    fails: u32,
    until: u64,
    key: Option<[u8; 32]>,
    reauth_until: Instant,
    rx: Option<Receiver<Msg>>,
    next_pull: Instant,
    last_input: Instant,
    hidden_at: Option<Instant>,
    /// The unlock PIN box has been focused for this appearance.
    pin_armed: bool,
    /// The Set-a-PIN field has been focused for this appearance.
    setup_focused: bool,
    /// Last unlock PIN already sent to a silent hash check.
    peeked: String,
    peek_tx: Option<mpsc::Sender<bool>>,
    peek_rx: Option<Receiver<bool>>,
}

impl Gate {
    pub fn new() -> Self {
        let file = applock::read_window_lock(&applock::window_lock_path());
        let verifier = open_cached(&file);
        Self {
            armed: false,
            ui: false,
            resetting: false,
            email: String::new(),
            token: String::new(),
            api: crate::cloud::default_api(),
            pin: String::new(),
            pin_show: false,
            confirm: String::new(),
            confirm_show: false,
            password: String::new(),
            pass_show: false,
            forgot: false,
            error: String::new(),
            note: String::new(),
            busy: false,
            verifier,
            sealed: file.sealed,
            updated: file.updated,
            fails: file.fails,
            until: file.until,
            key: None,
            reauth_until: Instant::now(),
            rx: None,
            next_pull: Instant::now(),
            last_input: Instant::now(),
            hidden_at: None,
            pin_armed: false,
            setup_focused: false,
            peeked: String::new(),
            peek_tx: None,
            peek_rx: None,
        }
    }

    pub fn blocking(&self) -> bool {
        self.ui && !self.email.is_empty()
    }

    /// Called when the window is created, and whenever the account is known.
    pub fn arm(&mut self, email: &str, token: &str, api: &str) {
        self.note_account(email, token, api);
        if self.email.is_empty() {
            return;
        }
        if !self.armed {
            self.armed = true;
            self.engage();
            tracing::info!("PJDesk Server window locked for {}", self.email);
        }
    }

    pub fn engage(&mut self) {
        if self.email.is_empty() {
            return;
        }
        self.ui = true;
        self.pin.clear();
        self.forgot = false;
        self.password.clear();
        self.pin_armed = false;
        self.setup_focused = false;
        self.error.clear();
        self.drop_peek();
    }

    fn drop_peek(&mut self) {
        self.peeked.clear();
        self.peek_tx = None;
        self.peek_rx = None;
    }

    /// Minimize, tray, and a second launch all come back through here.
    pub fn note_visibility(&mut self, ctx: &egui::Context, minimized: bool) {
        if self.email.is_empty() {
            self.ui = false;
            self.armed = false;
            return;
        }
        if !self.armed {
            self.armed = true;
            self.engage();
        }
        let idle = self.verifier.as_ref().map(|v| applock::clamp_idle(v.idle_sec)).unwrap_or(applock::IDLE_DEFAULT);
        if minimized {
            if self.hidden_at.is_none() {
                self.hidden_at = Some(Instant::now());
            }
            if self.hidden_at.is_some_and(|at| applock::relock(at.elapsed().as_secs(), idle)) {
                self.engage();
            }
            return;
        }
        if let Some(at) = self.hidden_at.take() {
            if applock::relock(at.elapsed().as_secs(), idle) {
                self.engage();
            }
        }
        if self.ui || self.verifier.is_none() {
            return;
        }
        if activity(ctx) {
            self.last_input = Instant::now();
        } else if applock::relock(self.last_input.elapsed().as_secs(), idle) {
            tracing::info!("PJDesk Server window locked after idle");
            self.engage();
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context, email: &str, token: &str, api: &str) {
        self.note_account(email, token, api);
        self.refresh_lockout();
        self.poll_jobs();
        if self.email.is_empty() || self.rx.is_some() || Instant::now() < self.next_pull || self.token.is_empty() {
            return;
        }
        self.next_pull = Instant::now() + Duration::from_secs(45);
        let token = self.token.clone();
        let api = self.api.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("app-lock-pull".into())
            .spawn(move || {
                let msg = fetch_lock(&api, &token)
                    .map(|remote| Msg::Remote(Ok(remote)))
                    .unwrap_or_else(|e| Msg::Remote(Err(e)));
                let _ = tx.send(msg);
            })
            .is_ok();
        if spawned {
            self.rx = Some(rx);
        }
        let _ = ctx;
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let gate = applock::window_gate(true, self.verifier.is_some() && !self.resetting);
        match gate {
            WindowGate::SetPin => self.setup_screen(ui),
            WindowGate::Unlock => self.unlock_screen(ui),
            WindowGate::Open => {}
        }
    }

    pub fn security_card(&mut self, ui: &mut egui::Ui) {
        let p = crate::ui_theme::paint(ui);
        ui.label(RichText::new("SECURITY").strong().color(p.muted));
        ui.add_space(4.0);
        if self.email.is_empty() {
            ui.label("Sign in with the PJDesk account. This window then asks for a PIN before it opens.");
            ui.add_space(8.0);
            return;
        }
        ui.label("A PIN is required to open this window and the PJDesk client. The service keeps running while the window is locked.");
        ui.add_space(8.0);
    }

    fn setup_screen(&mut self, ui: &mut egui::Ui) {
        let title = if self.resetting { "Choose a new PIN" } else { "Set a PIN" };
        let p = crate::ui_theme::paint(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(420.0);
                ui.add_space(72.0);
                ui.label(RichText::new(title).strong().size(22.0).color(p.text));
                ui.add_space(6.0);
                ui.label("This window stays on this screen until the PIN is saved to the PJDesk account. The service, incoming connections, and updates keep running.");
                ui.add_space(12.0);
                if self.key.is_none() && Instant::now() >= self.reauth_until {
                    ui.label("Account password");
                    let pass_box = secret_field(ui, &mut self.password, "pjdesk.com password", &mut self.pass_show);
                    if submitted(ui, &pass_box) {
                        self.setup_focused = false;
                    }
                    ui.add_space(8.0);
                }
                ui.label("PIN");
                let mut pin = self.pin.clone();
                let pin_box = secret_field(ui, &mut pin, "4 to 8 digits", &mut self.pin_show);
                if !self.setup_focused {
                    pin_box.request_focus();
                    self.setup_focused = true;
                }
                self.pin = digits(&pin);
                let advance = submitted(ui, &pin_box);
                ui.label("Confirm PIN");
                let mut confirm = self.confirm.clone();
                let confirm_box = secret_field(ui, &mut confirm, "Repeat the PIN", &mut self.confirm_show);
                if advance {
                    confirm_box.request_focus();
                }
                self.confirm = digits(&confirm);
                if submitted(ui, &confirm_box) && !self.busy {
                    self.start_save();
                }
                self.footer(ui, "Save PIN", true);
            });
        });
    }

    fn unlock_screen(&mut self, ui: &mut egui::Ui) {
        let now = unix_now();
        let waiting = self.until > now;
        if waiting {
            self.error = wait_label(self.until - now);
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        let p = crate::ui_theme::paint(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(420.0);
                ui.add_space(72.0);
                ui.label(RichText::new("Enter your PIN").strong().size(22.0).color(p.text));
                ui.add_space(6.0);
                ui.label(
                    "Your account stays signed in. This server stays online for incoming connections and updates.",
                );
                ui.add_space(12.0);
                let mut pin = self.pin.clone();
                let pin_box = secret_field(ui, &mut pin, "4 to 8 digits", &mut self.pin_show);
                if !self.pin_armed {
                    pin_box.request_focus();
                    self.pin_armed = true;
                }
                let prior = self.pin.clone();
                self.pin = digits(&pin);
                if self.pin != prior && !waiting {
                    self.error.clear();
                }
                self.peek_pin();
                let can = !self.busy && !waiting && applock::pin_acceptable(&self.pin);
                if submitted(ui, &pin_box) && can {
                    self.start_unlock();
                }
                self.footer(ui, "Unlock", false);
                ui.add_space(8.0);
                if ui.link("Forgot PIN? Sign in again").clicked() && !self.busy {
                    self.forgot = !self.forgot;
                    self.error.clear();
                }
                if self.forgot {
                    ui.add_space(8.0);
                    ui.label("This is your pjdesk.com password. After it is accepted you choose a new PIN.");
                    let pass_box = secret_field(ui, &mut self.password, "Account password", &mut self.pass_show);
                    ui.add_space(8.0);
                    let can_reset = !self.busy && !self.password.is_empty();
                    if submitted(ui, &pass_box) && can_reset {
                        self.start_reauth();
                    }
                    if crate::ui_widgets::accent_button(ui, "Sign in") && can_reset {
                        self.start_reauth();
                    }
                }
            });
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui, label: &str, setup: bool) {
        let p = crate::ui_theme::paint(ui);
        if !self.error.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(&self.error).color(p.warn));
        } else if !self.note.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(&self.note).color(p.muted).size(12.5));
        }
        ui.add_space(12.0);
        let now = unix_now();
        let waiting = !setup && self.until > now;
        let ready = if setup {
            applock::pin_acceptable(&self.pin) && self.pin == self.confirm
        } else {
            applock::pin_acceptable(&self.pin)
        };
        let shown = if self.busy { "Checking…" } else { label };
        if crate::ui_widgets::accent_button(ui, shown) && ready && !self.busy && !waiting {
            if setup {
                self.start_save();
            } else {
                self.start_unlock();
            }
        }
    }

    /// Hash the current PIN in the background. A match unlocks. A mismatch
    /// while typing is not a failed attempt and shows no error.
    fn peek_pin(&mut self) {
        if self.busy || self.resetting || !self.ui {
            return;
        }
        if self.until > unix_now() {
            return;
        }
        if !applock::pin_acceptable(&self.pin) || self.pin == self.peeked {
            return;
        }
        let Some(verifier) = self.verifier.clone() else { return };
        let pin = self.pin.clone();
        if self.peek_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.peek_tx = Some(tx);
            self.peek_rx = Some(rx);
        }
        let Some(tx) = self.peek_tx.clone() else { return };
        let spawned = std::thread::Builder::new()
            .name("app-lock-peek".into())
            .spawn(move || {
                let _ = tx.send(applock::matches(&verifier, &pin));
            })
            .is_ok();
        if spawned {
            self.peeked = self.pin.clone();
        }
    }

    fn poll_peek(&mut self) {
        let mut matched = false;
        let mut dead = false;
        if let Some(rx) = &self.peek_rx {
            loop {
                match rx.try_recv() {
                    Ok(true) => matched = true,
                    Ok(false) => {}
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
        }
        if dead {
            self.peek_rx = None;
            self.peek_tx = None;
        }
        if matched && self.ui && !self.resetting {
            self.unlock();
        }
    }

    fn start_unlock(&mut self) {
        let Some(verifier) = self.verifier.clone() else {
            self.error = "Set a PIN first.".into();
            return;
        };
        let pin = self.pin.clone();
        self.busy = true;
        self.error.clear();
        self.spawn(move || if applock::matches(&verifier, &pin) { Msg::Unlocked } else { Msg::Rejected });
    }

    fn start_reauth(&mut self) {
        let email = self.email.clone();
        let password = self.password.clone();
        let api = self.api.clone();
        self.busy = true;
        self.error.clear();
        self.spawn(move || match confirm_password(&api, &email, &password) {
            Ok(key) => Msg::Reauthed(key),
            Err(err) => Msg::Failed(err),
        });
    }

    fn start_save(&mut self) {
        if !applock::pin_acceptable(&self.pin) {
            self.note = "Use 4 to 8 digits.".into();
            return;
        }
        if self.pin != self.confirm {
            self.note = "Those PINs do not match.".into();
            return;
        }
        let recent = Instant::now() < self.reauth_until;
        if self.password.is_empty() && !recent && self.key.is_none() {
            self.note = "Enter your pjdesk.com password.".into();
            return;
        }
        if self.token.is_empty() {
            self.note = "Open this window as administrator once so the PIN can be saved to the account.".into();
            return;
        }
        let pin = self.pin.clone();
        let password = self.password.clone();
        let email = self.email.clone();
        let api = self.api.clone();
        let token = self.token.clone();
        let existing = self.key;
        let idle = self.verifier.as_ref().map(|v| v.idle_sec).unwrap_or(applock::IDLE_DEFAULT);
        self.busy = true;
        self.note = "Saving…".into();
        self.spawn(move || save_pin(&api, &token, &email, &password, existing, recent, &pin, idle));
    }

    fn spawn(&mut self, job: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("app-lock".into())
            .spawn(move || {
                let _ = tx.send(job());
            })
            .is_ok();
        if spawned {
            self.rx = Some(rx);
        } else {
            self.busy = false;
            self.error = "Could not save the PIN.".into();
        }
    }

    fn poll_jobs(&mut self) {
        self.poll_peek();
        let msg = self.rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Msg::Failed("Could not save the PIN.".into())),
        });
        let Some(msg) = msg else { return };
        self.rx = None;
        self.busy = false;
        self.apply(msg);
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Unlocked => self.unlock(),
            Msg::Rejected => {
                if !self.ui {
                    return;
                }
                let (fails, until) = applock::note_miss(self.fails, unix_now());
                self.fails = fails;
                self.until = until;
                self.pin.clear();
                self.peeked.clear();
                self.persist();
                self.error = if until > unix_now() {
                    wait_label(until - unix_now())
                } else {
                    let left = 5u32.saturating_sub(fails);
                    if left == 0 {
                        "Wrong PIN.".into()
                    } else {
                        format!("Wrong PIN. {left} left before a wait.")
                    }
                };
                tracing::info!("Wrong PIN on the PJDesk Server window ({fails} misses)");
            }
            Msg::Reauthed(key) => {
                self.key = Some(key);
                self.reauth_until = Instant::now() + Duration::from_secs(10 * 60);
                self.password.clear();
                self.forgot = false;
                self.resetting = true;
                self.ui = true;
                self.pin.clear();
                self.confirm.clear();
                self.setup_focused = false;
                self.error.clear();
                self.note = "Signed in. Choose a new PIN.".into();
                tracing::info!("Account password accepted. Waiting for a new PIN.");
            }
            Msg::Stored { sealed, updated, verifier, key } => {
                self.key = Some(key);
                self.sealed = sealed;
                self.updated = updated;
                self.verifier = Some(verifier);
                self.fails = 0;
                self.until = 0;
                self.resetting = false;
                self.persist();
                self.unlock();
                self.note = "PIN saved. This window will ask for it.".into();
                tracing::info!("Saved the account PIN from the PJDesk Server window");
            }
            Msg::Failed(err) => {
                tracing::info!("PIN was not saved: {err}");
                self.error = err.clone();
                self.note = err;
            }
            Msg::Remote(Ok(remote)) => self.apply_remote(remote),
            Msg::Remote(Err(err)) => {
                self.next_pull = Instant::now() + Duration::from_secs(60);
                if self.ui && self.verifier.is_none() {
                    self.note = err;
                }
            }
        }
    }

    fn apply_remote(&mut self, remote: Remote) {
        if remote.updated < self.updated {
            return;
        }
        if remote.updated == self.updated && remote.sealed == self.sealed {
            return;
        }
        if remote.sealed.trim().is_empty() {
            if remote.updated == 0 {
                return;
            }
            self.verifier = None;
            self.sealed.clear();
            self.updated = remote.updated;
            self.persist();
            self.ui = true;
            self.armed = true;
            tracing::info!("The account PIN was cleared. The window is asking for a new one.");
            return;
        }
        self.sealed = remote.sealed;
        self.updated = remote.updated;
        if let Some(key) = self.key {
            if let Ok(verifier) = applock::open(&key, self.sealed.trim()) {
                let changed = self.verifier.as_ref().map(|old| old.hash != verifier.hash).unwrap_or(true);
                self.verifier = Some(verifier);
                if changed {
                    self.fails = 0;
                    self.until = 0;
                    self.engage();
                }
            }
        }
        self.persist();
    }

    fn unlock(&mut self) {
        self.fails = 0;
        self.until = 0;
        self.ui = false;
        self.resetting = false;
        self.pin.clear();
        self.confirm.clear();
        self.error.clear();
        self.forgot = false;
        self.drop_peek();
        self.last_input = Instant::now();
        self.persist();
    }

    fn note_account(&mut self, email: &str, token: &str, api: &str) {
        let email = email.trim().to_lowercase();
        if !token.is_empty() {
            self.token = token.to_string();
        }
        if !api.is_empty() {
            self.api = api.trim_end_matches('/').to_string();
        }
        if email == self.email {
            return;
        }
        self.email = email;
        if self.email.is_empty() {
            self.ui = false;
            self.armed = false;
            return;
        }
        let file = applock::read_window_lock(&applock::window_lock_path());
        if file.email.eq_ignore_ascii_case(&self.email) {
            self.verifier = open_cached(&file);
            self.sealed = file.sealed;
            self.updated = file.updated;
            self.fails = file.fails;
            self.until = file.until;
        } else {
            self.verifier = None;
            self.sealed.clear();
            self.updated = 0;
            self.fails = 0;
            self.until = 0;
        }
        self.armed = false;
    }

    fn refresh_lockout(&mut self) {
        if self.email.is_empty() {
            return;
        }
        let file = applock::read_window_lock(&applock::window_lock_path());
        let (fails, until) = applock::lockout_for(&file, &self.email);
        if until > self.until {
            self.fails = fails;
            self.until = until;
        }
    }

    fn persist(&self) {
        let path = applock::window_lock_path();
        let mut file = applock::read_window_lock(&path);
        if !self.email.is_empty() {
            file.email = self.email.clone();
            file.fails = self.fails;
            file.until = self.until;
            file.sealed = self.sealed.clone();
            file.updated = self.updated;
            if let Some(verifier) = &self.verifier {
                if let Ok(text) = serde_json::to_string(verifier) {
                    file.verifier = protect(&text);
                }
            }
        }
        if let Err(err) = applock::write_window_lock(&path, &file) {
            tracing::info!("Could not save the window lock: {err}");
        }
    }
}

fn open_cached(file: &WindowLockFile) -> Option<Verifier> {
    if file.verifier.is_empty() {
        return None;
    }
    let text = reveal(&file.verifier)?;
    serde_json::from_str(&text).ok()
}

fn save_pin(
    api: &str,
    token: &str,
    email: &str,
    password: &str,
    existing: Option<[u8; 32]>,
    recent: bool,
    pin: &str,
    idle: u64,
) -> Msg {
    let key = if !password.is_empty() {
        match confirm_password(api, email, password) {
            Ok(key) => key,
            Err(err) => return Msg::Failed(err),
        }
    } else if recent || existing.is_some() {
        match existing {
            Some(key) => key,
            None => return Msg::Failed("Enter your pjdesk.com password.".into()),
        }
    } else {
        return Msg::Failed("Enter your pjdesk.com password.".into());
    };
    let verifier = match applock::create(pin, idle, unix_now().max(1)) {
        Ok(v) => v,
        Err(err) => return Msg::Failed(err),
    };
    let sealed = match applock::seal(&key, &verifier) {
        Ok(s) => s,
        Err(err) => return Msg::Failed(err),
    };
    match put_lock(api, token, &sealed) {
        Ok(updated) => Msg::Stored { sealed, updated, verifier, key },
        Err(err) => Msg::Failed(err),
    }
}

fn fetch_lock(api: &str, token: &str) -> Result<Remote, String> {
    let url = format!("{}/api/network/app-lock", api.trim_end_matches('/'));
    let text = post(&url, token, "{}")?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "the account sent an unexpected app lock".to_string())?;
    Ok(Remote {
        sealed: parsed.get("lock").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        updated: parsed.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

fn put_lock(api: &str, token: &str, lock: &str) -> Result<u64, String> {
    let url = format!("{}/api/network/app-lock", api.trim_end_matches('/'));
    let body = serde_json::json!({ "lock": lock }).to_string();
    let text = post(&url, token, &body)?;
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Ok(parsed.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0).max(1))
}

fn post(url: &str, token: &str, body: &str) -> Result<String, String> {
    match crate::cloud::agent()
        .post(url)
        .set("authorization", &format!("Bearer {token}"))
        .set("content-type", "application/json")
        .send_string(body)
    {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp
                .into_string()
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(str::to_string))
                .unwrap_or_else(|| format!("PJDesk Cloud error (HTTP {code})"));
            Err(msg)
        }
        Err(ureq::Error::Transport(err)) => Err(format!("Could not reach pjdesk.com. {err}")),
    }
}

fn confirm_password(api: &str, email: &str, password: &str) -> Result<[u8; 32], String> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        return Err("Enter your account password.".into());
    }
    let keys = crate::cloud::derive_account_keys(&email, password);
    let body = serde_json::json!({
        "email": email,
        "auth_key": hex::encode(keys.auth),
        "device": "PJDesk Server PIN check",
    })
    .to_string();
    let url = format!("{}/api/login", api.trim_end_matches('/'));
    let text = match crate::cloud::agent().post(&url).set("content-type", "application/json").send_string(&body) {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(crate::cloud::cloud_error_text(code, &text));
        }
        Err(ureq::Error::Transport(err)) => return Err(format!("Could not reach pjdesk.com. {err}")),
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "unexpected reply from PJDesk".to_string())?;
    let token = parsed.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if !token.is_empty() {
        let _ = crate::cloud::agent()
            .post(&format!("{}/api/logout", api.trim_end_matches('/')))
            .set("authorization", &format!("Bearer {token}"))
            .send_string("{}");
    }
    Ok(pjdesk_proto::access::derive_password_vault_key(&email, password))
}

fn submitted(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
}

fn secret_field(ui: &mut egui::Ui, value: &mut String, hint: &str, shown: &mut bool) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let width = (ui.available_width() - 72.0).max(48.0);
        let response = ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .password(!*shown)
                .desired_width(width)
                .margin(egui::Margin::symmetric(12, 10)),
        );
        crate::ui_widgets::reveal(ui, shown);
        response
    })
    .inner
}

fn digits(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_digit()).take(8).collect()
}

fn activity(ctx: &egui::Context) -> bool {
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

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

fn protect(secret: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(bytes) = dpapi::protect(secret.as_bytes()) {
            return format!("dpapi:{}", hex::encode(bytes));
        }
    }
    format!("plain:{}", hex::encode(secret.as_bytes()))
}

fn reveal(stored: &str) -> Option<String> {
    if let Some(hexed) = stored.strip_prefix("plain:") {
        return hex::decode(hexed).ok().and_then(|b| String::from_utf8(b).ok());
    }
    #[cfg(windows)]
    if let Some(hexed) = stored.strip_prefix("dpapi:") {
        let bytes = hex::decode(hexed).ok()?;
        return dpapi::unprotect(&bytes).and_then(|b| String::from_utf8(b).ok());
    }
    None
}

#[cfg(windows)]
mod dpapi {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    fn as_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let out = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut _)));
        out
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = as_blob(data);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(&input, None, None, None, None, 0x1, &mut output).ok()?;
            Some(take(output))
        }
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = as_blob(data);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, 0x1, &mut output).ok()?;
            Some(take(output))
        }
    }
}
