//! The panels that make up the main window, as methods on [`super::App`].

use eframe::egui::{self, Color32, RichText};
use pjdesk_proto::signal::AddressEntry;
use pjdesk_proto::{format_id, normalize_id};

use super::{theme, widgets, App};

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(logo.id(), egui::vec2(24.0, 24.0))));
                ui.add_space(2.0);
            } else {
                ui.label(RichText::new("◆").size(16.0).color(theme::ACCENT));
            }
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
                        let who = if acct.display_name.is_empty() {
                            acct.username.clone()
                        } else {
                            acct.display_name.clone()
                        };
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

            // The one-time password is always available and shown here (you read
            // it out to whoever is connecting). A permanent password, if set,
            // also works but is never shown.
            ui.label(RichText::new("One-time password").color(theme::MUTED).size(12.0));
            ui.horizontal(|ui| {
                let shown = if password.is_empty() {
                    "…".to_string()
                } else if self.show_password {
                    password.clone()
                } else {
                    "•".repeat(password.chars().count().max(6))
                };
                ui.label(RichText::new(shown).size(20.0).monospace().color(Color32::WHITE));
                if !password.is_empty() {
                    let eye = if self.show_password { "🙈" } else { "👁" };
                    if ui.button(eye).on_hover_text(if self.show_password { "Hide" } else { "Reveal" }).clicked() {
                        self.show_password = !self.show_password;
                    }
                    if ui.button("⧉").on_hover_text("Copy password").clicked() && widgets::copy_to_clipboard(&password)
                    {
                        self.toast.show("Password copied");
                    }
                    if ui.button("🔄").on_hover_text("New one-time password").clicked() {
                        shared.request_regenerate();
                    }
                }
            });

            let permanent = self.cfg.settings.permanent_password.is_some();
            self.permanent_password_row(ui, permanent);

            ui.add_space(10.0);
            ui.label(
                RichText::new("Give your ID and the one-time password to let someone in. A permanent password, if set, works too.")
                    .color(theme::MUTED)
                    .size(12.0),
            );
            self.chat_box(ui);
        });
    }

    fn chat_box(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("Chat during a session").color(theme::MUTED).size(12.0));
        let lines = self.engine.shared.chat();
        let start = lines.len().saturating_sub(6);
        for (mine, line) in lines.iter().skip(start) {
            let who = if *mine { "You" } else { "Them" };
            ui.label(format!("{who}: {line}"));
        }
        ui.horizontal(|ui| {
            let resp = ui.text_edit_singleline(&mut self.chat_draft);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Send").clicked() || enter) && !self.chat_draft.trim().is_empty() {
                let text = self.chat_draft.trim().to_string();
                self.engine.shared.send_chat(text);
                self.chat_draft.clear();
            }
        });
    }

    /// A compact permanent-password control, shared by both share panels. When
    /// one is set it shows a Remove button; otherwise a field to set one. The
    /// permanent password works in addition to the one-time password.
    fn permanent_password_row(&mut self, ui: &mut egui::Ui, is_set: bool) {
        ui.add_space(8.0);
        if is_set {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔒 Permanent password set").color(theme::OK).size(12.0));
                if ui.small_button("Remove").on_hover_text("Stop accepting the permanent password").clicked() {
                    self.remove_permanent_password();
                }
            });
        } else {
            ui.label(
                RichText::new("Permanent password (optional, for unattended access)").color(theme::MUTED).size(12.0),
            );
            ui.horizontal(|ui| {
                let resp = widgets::password_field(
                    ui,
                    &mut self.perm_input,
                    "at least 8 characters",
                    &mut self.show_perm_input,
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Set").clicked() || enter) && !self.perm_input.trim().is_empty() {
                    let pw = self.perm_input.clone();
                    self.set_permanent_password(&pw);
                }
            });
        }
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
            ui.label(RichText::new("One-time password").color(theme::MUTED).size(12.0));
            ui.horizontal(|ui| match &one_time_pw {
                Some(pw) => {
                    let shown = if self.show_password { pw.clone() } else { "•".repeat(pw.chars().count().max(6)) };
                    ui.label(RichText::new(shown).size(20.0).monospace().color(Color32::WHITE));
                    let eye = if self.show_password { "🙈" } else { "👁" };
                    if ui.button(eye).on_hover_text(if self.show_password { "Hide" } else { "Reveal" }).clicked() {
                        self.show_password = !self.show_password;
                    }
                    if ui.button("⧉").on_hover_text("Copy password").clicked() && widgets::copy_to_clipboard(pw) {
                        self.toast.show("Password copied");
                    }
                }
                None => {
                    ui.label(RichText::new("…").size(20.0).monospace().color(theme::MUTED));
                }
            });

            self.permanent_password_row(ui, status.permanent_set);

            ui.add_space(10.0);
            ui.label(
                RichText::new("Shared by the background service, so it's reachable even at the sign-in screen. Give your ID and the one-time password to let someone in; a permanent password, if set, works too.")
                    .color(theme::MUTED)
                    .size(12.0),
            );
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
            widgets::password_field(ui, &mut self.connect_password, "password", &mut self.show_connect_password);
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

            ui.add_space(12.0);
            widgets::field_label(ui, "Or connect directly");
            widgets::text_field(ui, &mut self.direct_addr, "192.168.1.20", false, false);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Connect to address").clicked() {
                    let addr = self.direct_addr.clone();
                    self.connect_direct(&addr);
                }
                if ui.button("Find on this network").clicked() {
                    self.find_on_lan();
                }
            });
            for (name, id, addr) in self.lan.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!("{name}  {}", pjdesk_proto::format_id(&id)));
                    if ui.button("Connect").clicked() {
                        self.direct_addr = addr.clone();
                        self.connect_direct(&addr);
                    }
                });
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
            ui.label(
                RichText::new(
                    "No saved computers yet. Connect to one and choose \u{201c}Add to saved computers\u{201d}.",
                )
                .color(theme::MUTED),
            );
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
                            let sub = if entry.alias.is_empty() { String::new() } else { format_id(&entry.id) };
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
                let pw = widgets::password_field(ui, &mut self.login_pass, "", &mut self.show_login_pass);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let clicked = ui
                        .add_enabled(
                            !self.login_busy,
                            egui::Button::new(if self.login_busy { "Signing in…" } else { "Sign in" })
                                .fill(theme::ACCENT),
                        )
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

    pub(super) fn change_password_window(&mut self, ctx: &egui::Context) {
        if !self.show_change {
            return;
        }
        let mut open = true;
        let mut submit = false;
        egui::Window::new("Change your password")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(RichText::new(&self.password_notice).color(theme::WARN));
                ui.add_space(8.0);
                ui.label("New password");
                widgets::password_field(ui, &mut self.change_pass, "at least 12 characters", &mut self.show_change_pass);
                ui.add_space(4.0);
                ui.label("Type it again");
                let mut shown = self.show_change_pass;
                widgets::password_field(ui, &mut self.change_pass2, "", &mut shown);
                self.show_change_pass = shown;
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Change password").clicked() {
                        submit = true;
                    }
                    if ui.button("Not now").clicked() {
                        self.show_change = false;
                    }
                });
            });
        if !open {
            self.show_change = false;
        }
        if submit {
            if self.change_rx.is_some() {
                return;
            }
            if self.change_pass != self.change_pass2 {
                self.status = "The two passwords do not match.".into();
                return;
            }
            let new_password = self.change_pass.clone();
            let Some(server) = self.server_addr() else {
                self.status = "Set your server in Settings first.".into();
                return;
            };
            self.status = "Updating the password…".into();
            let (tx, rx) = std::sync::mpsc::channel();
            self.change_rx = Some(rx);
            let mut cfg = self.cfg.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                let result = rt.block_on(crate::account::change_password(&server, &mut cfg, &new_password));
                let _ = tx.send(result.map(|_| ()).map_err(|e| format!("{e:#}")));
            });
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
