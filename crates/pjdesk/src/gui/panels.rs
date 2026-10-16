//! The panels that make up the main window, as methods on [`super::App`].

use pjdesk_proto::signal::AddressEntry;
use pjdesk_proto::{format_id, normalize_id};
use eframe::egui::{self, Color32, RichText};

use super::{theme, widgets, App};

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new("◆").size(16.0).color(theme::ACCENT));
            ui.label(RichText::new("PJDesk").size(20.0).strong().color(Color32::WHITE));
            ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(theme::MUTED).size(12.0));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙  Settings").clicked() {
                    self.settings = super::settings::SettingsDraft::from_config(&self.cfg);
                    self.show_settings = true;
                }
                ui.separator();
                match &self.cfg.account {
                    Some(acct) => {
                        let who = if acct.display_name.is_empty() { acct.username.clone() } else { acct.display_name.clone() };
                        if ui.button("Sign out").clicked() {
                            self.sign_out();
                        }
                        ui.label(RichText::new(format!("👤 {who}")).color(theme::OK));
                    }
                    None => {
                        if ui.button("Sign in").clicked() {
                            self.show_login = true;
                            self.status.clear();
                        }
                    }
                }
            });
        });
        ui.add_space(6.0);
    }

    /// Left panel: share this computer.
    pub(super) fn host_panel(&mut self, ui: &mut egui::Ui) {
        if self.engine.service_running {
            self.service_host_panel(ui);
            return;
        }
        theme::card(ui, |ui| {
            widgets::heading(ui, "SHARE THIS COMPUTER");
            ui.add_space(4.0);

            let shared = &self.engine.shared;
            let online = shared.is_online();
            let id = shared.id();
            let password = shared.password();
            let connected = shared.connected_peer();
            let error = shared.error();

            // Status line.
            if self.cfg.server.is_none() {
                widgets::status_dot(ui, theme::WARN, "No server set — open Settings to go online");
            } else if let Some(peer) = &connected {
                widgets::status_dot(ui, theme::ACCENT, &format!("Connected: {peer} is controlling this computer"));
            } else if online {
                widgets::status_dot(ui, theme::OK, "Ready for a connection");
            } else if let Some(e) = &error {
                widgets::status_dot(ui, theme::WARN, &short(e));
            } else {
                widgets::status_dot(ui, theme::MUTED, "Connecting to your server…");
            }

            ui.add_space(12.0);

            // ID.
            ui.label(RichText::new("Your ID").color(theme::MUTED).size(12.0));
            ui.horizontal(|ui| {
                let shown = id.as_deref().map(format_id).unwrap_or_else(|| "— — —".into());
                ui.label(RichText::new(shown).size(32.0).strong().monospace().color(Color32::WHITE));
                if id.is_some() && ui.button("⧉").on_hover_text("Copy ID").clicked() {
                    if widgets::copy_to_clipboard(&normalize_id(id.as_deref().unwrap_or(""))) {
                        self.toast.show("ID copied");
                    }
                }
            });

            ui.add_space(8.0);

            // A permanent password is one you set yourself in Settings, so it's
            // never revealed here — only ever masked, and it can't be copied off
            // this screen. The one-time password can be revealed, since you have
            // to read it out to whoever is connecting.
            let permanent = self.cfg.settings.permanent_password.is_some();
            ui.label(RichText::new(if permanent { "Password (permanent)" } else { "One-time password" }).color(theme::MUTED).size(12.0));
            ui.horizontal(|ui| {
                let shown = if password.is_empty() {
                    "…".to_string()
                } else if permanent {
                    "•".repeat(10)
                } else if self.show_password {
                    password.clone()
                } else {
                    "•".repeat(password.chars().count().max(6))
                };
                ui.label(RichText::new(shown).size(20.0).monospace().color(Color32::WHITE));
                if !password.is_empty() && !permanent {
                    let eye = if self.show_password { "🙈" } else { "👁" };
                    if ui.button(eye).on_hover_text(if self.show_password { "Hide" } else { "Reveal" }).clicked() {
                        self.show_password = !self.show_password;
                    }
                    if ui.button("⧉").on_hover_text("Copy password").clicked() && widgets::copy_to_clipboard(&password) {
                        self.toast.show("Password copied");
                    }
                    if ui.button("🔄").on_hover_text("New random password").clicked() {
                        shared.request_regenerate();
                    }
                }
            });
            if permanent {
                ui.label(RichText::new("Hidden for security — set or change it in Settings ▸ Security.").color(theme::MUTED).size(11.0));
                ui.add_space(5.0);
                if ui.button("Use a one-time password instead").clicked() {
                    self.use_one_time_password();
                }
            }

            ui.add_space(10.0);
            ui.label(
                RichText::new("Give your ID and password to someone to let them control this computer.")
                    .color(theme::MUTED)
                    .size(12.0),
            );
        });
    }

    /// Left panel when the background service is doing the hosting.
    fn service_host_panel(&mut self, ui: &mut egui::Ui) {
        let status = self.svc_status.clone();
        let fresh = status.is_fresh();
        theme::card(ui, |ui| {
            widgets::heading(ui, "SHARE THIS COMPUTER");
            ui.add_space(4.0);

            if let Some(peer) = status.connected.filter(|_| fresh) {
                widgets::status_dot(ui, theme::ACCENT, &format!("Connected: {peer} is controlling this computer"));
            } else if fresh && status.online {
                widgets::status_dot(ui, theme::OK, "Ready — shared by the background service");
            } else {
                widgets::status_dot(ui, theme::MUTED, "Background service is starting…");
            }

            ui.add_space(12.0);
            ui.label(RichText::new("Your ID").color(theme::MUTED).size(12.0));
            ui.horizontal(|ui| {
                let shown = status.id.as_deref().map(format_id).unwrap_or_else(|| "— — —".into());
                ui.label(RichText::new(shown).size(32.0).strong().monospace().color(Color32::WHITE));
                if let Some(id) = &status.id {
                    if ui.button("⧉").on_hover_text("Copy ID").clicked() && widgets::copy_to_clipboard(id) {
                        self.toast.show("ID copied");
                    }
                }
            });

            ui.add_space(8.0);
            let one_time_pw = status.password.clone().filter(|p| !p.is_empty());
            ui.label(
                RichText::new(if one_time_pw.is_some() { "One-time password" } else { "Password" })
                    .color(theme::MUTED)
                    .size(12.0),
            );
            ui.horizontal(|ui| match &one_time_pw {
                Some(pw) => {
                    let shown =
                        if self.show_password { pw.clone() } else { "•".repeat(pw.chars().count().max(6)) };
                    ui.label(RichText::new(shown).size(20.0).monospace().color(Color32::WHITE));
                    let eye = if self.show_password { "🙈" } else { "👁" };
                    if ui.button(eye).on_hover_text(if self.show_password { "Hide" } else { "Reveal" }).clicked() {
                        self.show_password = !self.show_password;
                    }
                    if ui.button("⧉").on_hover_text("Copy password").clicked() && widgets::copy_to_clipboard(pw) {
                        self.toast.show("Password copied");
                    }
                }
                None if fresh && status.online => {
                    ui.label(RichText::new("•".repeat(10)).size(20.0).monospace().color(Color32::WHITE));
                }
                None => {
                    ui.label(RichText::new("…").size(20.0).monospace().color(theme::MUTED));
                }
            });

            ui.add_space(10.0);
            let note = if one_time_pw.is_some() {
                "Shared by the background service, so it's reachable even at the sign-in screen. Give your ID and this one-time password to let someone in — it changes when the service restarts."
            } else {
                "Shared by the background service, so it's reachable even at the sign-in screen. A permanent password is set — change it in Settings ▸ Security."
            };
            ui.label(RichText::new(note).color(theme::MUTED).size(12.0));
            if one_time_pw.is_none() && fresh && status.online {
                ui.add_space(5.0);
                if ui.button("Use a one-time password instead").clicked() {
                    self.use_one_time_password();
                }
            }
        });
    }

    /// Right panel: control another computer.
    pub(super) fn control_panel(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            widgets::heading(ui, "CONTROL ANOTHER COMPUTER");
            ui.add_space(4.0);

            widgets::field_label(ui, "Remote ID");
            let resp = widgets::text_field(ui, &mut self.connect_id, "123 456 789", false, true);

            ui.add_space(10.0);
            widgets::field_label(ui, "Password (optional if saved)");
            widgets::text_field(ui, &mut self.connect_password, "password", true, false);
            ui.add_space(10.0);
            ui.checkbox(&mut self.remember, "Remember this computer's password");

            ui.add_space(14.0);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let connect = widgets::primary_button(ui, "Connect");
            if connect || enter {
                let (id, pw) = (self.connect_id.clone(), self.connect_password.clone());
                self.connect_to(&id, Some(pw));
                self.connect_password.clear();
            }

            ui.add_space(8.0);
            if ui.button("＋ Add to saved computers").clicked() {
                self.book_target = Some(normalize_id(&self.connect_id));
                self.book_alias.clear();
            }
        });
    }

    /// Saved computers (address book), below the two panels.
    pub(super) fn address_book(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            widgets::heading(ui, "SAVED COMPUTERS");
            if self.cfg.account.is_some() {
                ui.label(RichText::new("· synced to your account").color(theme::MUTED).size(11.0));
            }
        });

        if self.cfg.address_book.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("No saved computers yet. Connect to one and choose \u{201c}Add to saved computers\u{201d}.").color(theme::MUTED));
            return;
        }

        let mut connect: Option<(String, Option<String>)> = None;
        let mut remove: Option<String> = None;
        let book = self.cfg.address_book.clone();

        egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
            for entry in &book {
                theme::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let title = if entry.alias.is_empty() { format_id(&entry.id) } else { entry.alias.clone() };
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong().color(Color32::WHITE));
                            let sub = if entry.alias.is_empty() {
                                String::new()
                            } else {
                                format_id(&entry.id)
                            };
                            let saved = if entry.password.is_some() { "  🔑 password saved" } else { "" };
                            ui.label(RichText::new(format!("{sub}{saved}")).color(theme::MUTED).size(12.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("🗑").on_hover_text("Remove").clicked() {
                                remove = Some(entry.id.clone());
                            }
                            if ui.add(egui::Button::new(RichText::new("Connect").strong())).clicked() {
                                connect = Some((entry.id.clone(), entry.password.clone()));
                            }
                        });
                    });
                });
            }
        });

        if let Some((id, pw)) = connect {
            self.connect_to(&id, pw);
        }
        if let Some(id) = remove {
            self.cfg.address_book.retain(|e| e.id != id);
            self.cfg.remember_password(&id, None);
            self.sync_address_book();
            self.toast.show("Removed");
        }
    }

    pub(super) fn add_book_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.book_target.clone() else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Add to saved computers")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Name (optional)");
                ui.text_edit_singleline(&mut self.book_alias);
                ui.add_space(6.0);
                ui.label("ID");
                let mut id = target.clone();
                ui.add_enabled(false, egui::TextEdit::singleline(&mut id));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        save = false;
                        self.book_target = None;
                    }
                    if ui.add(egui::Button::new("Save").fill(theme::ACCENT)).clicked() {
                        save = true;
                    }
                });
            });
        if !open {
            self.book_target = None;
        }
        if save {
            let id = normalize_id(&target);
            if pjdesk_proto::is_valid_id(&id) {
                if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == id) {
                    entry.alias = self.book_alias.trim().to_string();
                } else {
                    self.cfg.address_book.push(AddressEntry {
                        id,
                        alias: self.book_alias.trim().to_string(),
                        ..Default::default()
                    });
                }
                self.sync_address_book();
                self.toast.show("Saved");
            } else {
                self.status = "That isn't a 9-digit PJDesk ID.".into();
            }
            self.book_target = None;
        }
    }

    pub(super) fn login_window(&mut self, ctx: &egui::Context) {
        if !self.show_login {
            return;
        }
        let mut open = true;
        let mut submit = false;
        egui::Window::new("Sign in to your server")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if self.cfg.server.is_none() {
                    ui.colored_label(theme::WARN, "Set your server in Settings first.");
                }
                ui.label("Username");
                ui.text_edit_singleline(&mut self.login_user);
                ui.add_space(4.0);
                ui.label("Password");
                let pw = ui.add(egui::TextEdit::singleline(&mut self.login_pass).password(true));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let clicked = ui
                        .add_enabled(!self.login_busy, egui::Button::new(if self.login_busy { "Signing in…" } else { "Sign in" }).fill(theme::ACCENT))
                        .clicked();
                    let entered = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    submit = clicked || entered;
                    if ui.button("Cancel").clicked() {
                        self.show_login = false;
                    }
                });
            });
        if !open {
            self.show_login = false;
        }
        if submit && !self.login_busy {
            let (tx, rx) = std::sync::mpsc::channel();
            self.login_rx = Some(rx);
            self.start_login(tx);
        }
    }
}

/// Trims a long error to one tidy line for the status dot.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text);
    if line.chars().count() > 80 {
        let truncated: String = line.chars().take(79).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}
