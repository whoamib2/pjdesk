//! The main window: a left rail for this computer, and the remote desktop beside it.

use eframe::egui::{self, RichText, ScrollArea, Vec2};
use pjdesk_proto::signal::AddressEntry;
use pjdesk_proto::{format_id, normalize_id};

use super::icons::Icon;
use super::{theme, widgets, App};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Connect,
    Files,
    Favorite,
    Forget,
    Remove,
    CopyId,
}

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(logo.id(), egui::vec2(28.0, 28.0))));
            } else {
                theme::icon_in(ui, Icon::Monitor, 22.0, p.accent);
            }
            ui.label(RichText::new("PJDesk").size(20.0).strong().color(p.text));
            ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(p.muted).size(12.5));
            ui.add_space(12.0);
            let connect_on = self.page == super::MainPage::Connect;
            let saved_on = self.page == super::MainPage::Saved;
            if ui.selectable_label(connect_on, "Connect").clicked() {
                self.page = super::MainPage::Connect;
            }
            if ui.selectable_label(saved_on, "Saved computers").clicked() {
                self.page = super::MainPage::Saved;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Gear, "Settings (Ctrl+,)").clicked() {
                    self.open_settings();
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
                        theme::icon_in(ui, Icon::User, 16.0, p.ok);
                        ui.label(RichText::new(who).color(p.ok));
                    }
                    None => {
                        if ui.button("Sign in").on_hover_text("Sign in to your PJDesk server").clicked() {
                            self.show_login = true;
                            self.status.clear();
                        }
                        theme::icon_in(ui, Icon::User, 16.0, p.muted);
                    }
                }
            });
        });
    }

    pub(super) fn body(&mut self, ui: &mut egui::Ui) {
        if self.page == super::MainPage::Saved {
            self.saved_page(ui);
            return;
        }
        let width = ui.available_width();
        let height = ui.available_height();
        if width < 900.0 {
            ScrollArea::vertical().id_salt("stacked").auto_shrink([false, false]).show(ui, |ui| {
                self.rail(ui);
                ui.add_space(12.0);
                self.remote(ui);
            });
            return;
        }
        let gap = 12.0;
        let left = 360.0;
        let right = (width - left - gap).max(320.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(left, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ScrollArea::vertical().id_salt("rail").auto_shrink([false, false]).show(ui, |ui| {
                    self.rail(ui);
                });
            });
            ui.add_space(gap);
            ui.allocate_ui_with_layout(Vec2::new(right, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ScrollArea::vertical().id_salt("remote").auto_shrink([false, false]).show(ui, |ui| {
                    self.remote(ui);
                });
            });
        });
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        theme::card(ui, |ui| {
            widgets::heading(ui, "YOUR DESKTOP");
            ui.add_space(4.0);
            self.share_status(ui);
            ui.add_space(12.0);

            let (id, password, permanent, in_process) = self.share_secrets();
            ui.label(RichText::new("Your ID").color(p.muted).size(12.5));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let shown = id.as_deref().map(format_id).unwrap_or_else(|| "— — —".into());
                ui.label(RichText::new(shown).size(32.0).strong().color(p.text));
                if let Some(id) = id.clone() {
                    if widgets::icon_button(ui, Icon::Copy, "Copy ID").clicked()
                        && widgets::copy_to_clipboard(&normalize_id(&id))
                    {
                        self.toast.show("ID copied");
                    }
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new("One-time password").color(p.muted).size(12.5));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                match &password {
                    Some(pw) => {
                        let shown =
                            if self.show_password { pw.clone() } else { "•".repeat(pw.chars().count().max(6)) };
                        ui.label(RichText::new(shown).size(20.0).monospace().color(p.text));
                        let eye = if self.show_password { Icon::EyeOff } else { Icon::Eye };
                        let tip = if self.show_password { "Hide password" } else { "Show password" };
                        if widgets::icon_button(ui, eye, tip).clicked() {
                            self.show_password = !self.show_password;
                        }
                        if widgets::icon_button(ui, Icon::Copy, "Copy password").clicked()
                            && widgets::copy_to_clipboard(pw)
                        {
                            self.toast.show("Password copied");
                        }
                        if in_process && widgets::icon_button(ui, Icon::Refresh, "New one-time password").clicked() {
                            self.engine.shared.request_regenerate();
                            self.toast.show("New one-time password");
                        }
                    }
                    None => {
                        widgets::spinner(ui, p.muted);
                        ui.label(RichText::new("Waiting for a password").color(p.muted));
                    }
                }
            });

            self.permanent_password_row(ui, permanent);
            ui.add_space(8.0);
            ui.label(widgets::muted(
                ui,
                "Give your ID and the one-time password to let someone in. A permanent password, if set, works too — at least 8 characters.",
            ));
            if in_process {
                self.chat_box(ui);
            }
        });
    }

    fn share_status(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        if !self.engine.ready {
            ui.horizontal(|ui| {
                widgets::spinner(ui, p.muted);
                widgets::status_dot(ui, p.muted, "Starting…");
            });
            return;
        }
        let server = server_host(&self.cfg.effective_server());
        let rtt = self.share_rtt();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if self.engine.service_running {
                let fresh = self.svc_status.is_fresh();
                if self.svc_status.connected.is_some() && fresh {
                    widgets::status_dot(ui, p.accent, "In a session");
                } else if fresh && self.svc_status.online {
                    widgets::status_dot(ui, p.ok, "Service ready");
                } else {
                    widgets::status_dot(ui, p.muted, "Service starting");
                }
            } else if self.cfg.server.is_none() && self.cfg.effective_server().is_empty() {
                widgets::status_dot(ui, p.warn, "No server set");
            } else if let Some(peer) = self.engine.shared.connected_peer() {
                widgets::status_dot(ui, p.accent, &format!("In a session · {peer}"));
            } else if self.engine.shared.is_online() {
                widgets::status_dot(ui, p.ok, "Ready");
            } else if let Some(err) = self.engine.shared.error() {
                widgets::status_dot(ui, p.warn, &short(&err));
            } else {
                widgets::status_dot(ui, p.muted, "Connecting");
            }
            widgets::status_dot(ui, p.muted, &server);
            if rtt > 0 {
                widgets::status_dot(ui, p.muted, &format!("{rtt} ms"));
            }
        });
    }

    /// ID, one-time password, whether a permanent password is set, and whether
    /// this process (rather than the background service) is hosting.
    fn share_secrets(&self) -> (Option<String>, Option<String>, bool, bool) {
        if self.engine.service_running {
            let pw = self.svc_status.password.clone().filter(|p| !p.is_empty());
            (self.svc_status.id.clone(), pw, self.svc_status.permanent_set, false)
        } else {
            let pw = {
                let pw = self.engine.shared.password();
                if pw.is_empty() {
                    None
                } else {
                    Some(pw)
                }
            };
            (
                self.engine.shared.id(),
                pw,
                self.cfg.settings.permanent_password.is_some() || self.engine.shared.permanent_set(),
                true,
            )
        }
    }

    fn permanent_password_row(&mut self, ui: &mut egui::Ui, is_set: bool) {
        let p = theme::paint(ui);
        ui.add_space(8.0);
        if is_set {
            ui.horizontal(|ui| {
                theme::icon_in(ui, Icon::Lock, 14.0, p.ok);
                ui.label(RichText::new("Permanent password set").color(p.ok).size(12.5));
                if ui.small_button("Remove").on_hover_text("Stop accepting the permanent password").clicked() {
                    self.remove_permanent_password();
                }
            });
        } else {
            ui.label(widgets::muted(ui, "Permanent password (optional, for unattended access)"));
            widgets::password_field(
                ui,
                &mut self.perm_input,
                widgets::PERMANENT_PASSWORD_HINT,
                &mut self.show_perm_input,
            );
            if ui.add_sized([88.0, 36.0], egui::Button::new("Set")).on_hover_text("At least 8 characters").clicked()
                && !self.perm_input.trim().is_empty()
            {
                let pw = self.perm_input.clone();
                self.set_permanent_password(&pw);
            }
        }
    }

    fn chat_box(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(widgets::muted(ui, "Chat during a session"));
        let lines = self.engine.shared.chat();
        let start = lines.len().saturating_sub(4);
        for (mine, line) in lines.iter().skip(start) {
            let who = if *mine { "You" } else { "Them" };
            ui.label(format!("{who}: {line}"));
        }
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.chat_draft)
                    .hint_text("Message")
                    .desired_width(ui.available_width() - 72.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Send").clicked() || enter) && !self.chat_draft.trim().is_empty() {
                let text = self.chat_draft.trim().to_string();
                self.engine.shared.send_chat(text);
                self.chat_draft.clear();
            }
        });
    }

    fn remote(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        theme::card(ui, |ui| {
            widgets::heading(ui, "CONTROL REMOTE DESKTOP");
            ui.add_space(4.0);
            widgets::field_label(ui, "Remote ID");
            let id_resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let menu_w = 96.0;
                    let width = (ui.available_width() - menu_w - 8.0).max(80.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.connect_id)
                            .hint_text("123 456 789")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(width)
                            .margin(egui::Margin::symmetric(12, 10)),
                    );
                    let recent = self.recent_ids();
                    ui.menu_button("Recent", |ui| {
                        if recent.is_empty() {
                            ui.label(widgets::muted(ui, "No recent computers"));
                        }
                        for id in recent {
                            let label = self.row_title(&id);
                            if ui.button(label).clicked() {
                                self.connect_id = format_typed_id(&id);
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Recent computers");
                    resp
                })
                .inner;
            if id_resp.changed() {
                let formatted = format_typed_id(&self.connect_id);
                if formatted != self.connect_id {
                    self.connect_id = formatted;
                }
            }

            ui.add_space(8.0);
            widgets::field_label(ui, "Password (optional if saved)");
            let pw_resp =
                widgets::password_field(ui, &mut self.connect_password, "Password", &mut self.show_connect_password);
            ui.add_space(4.0);
            widgets::checkbox(ui, &mut self.remember, "Remember this computer's password");

            ui.add_space(8.0);
            let enter = (id_resp.lost_focus() || pw_resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut connect = false;
            let mut files = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let total = ui.available_width();
                let primary = (total * 0.58).max(120.0);
                let secondary = (total - primary - 8.0).max(120.0);
                connect = widgets::accent_button(ui, "Connect", Vec2::new(primary, 40.0));
                files = ui
                    .add_sized([secondary, 40.0], egui::Button::new(RichText::new("Transfer file").strong()))
                    .on_hover_text("Connect and open file transfer")
                    .clicked();
            });
            if connect || files || enter {
                let (id, pw) = (self.connect_id.clone(), self.connect_password.clone());
                self.connect_to(&id, Some(pw), files);
            }

            ui.add_space(12.0);
            widgets::field_label(ui, "Or connect directly");
            widgets::text_field(ui, &mut self.direct_addr, "192.168.1.20", false);
            ui.horizontal(|ui| {
                if ui.button("Connect to address").clicked() {
                    let addr = self.direct_addr.clone();
                    self.connect_direct(&addr);
                }
                if self.lan_rx.is_some() {
                    widgets::spinner(ui, p.muted);
                    ui.label(widgets::muted(ui, "Looking on this network…"));
                } else if ui.button("Find on this network").clicked() {
                    self.find_on_lan();
                    self.desk_tab = super::DeskTab::Lan;
                }
            });
        });

        ui.add_space(12.0);
        self.desk_tabs(ui);
    }

    fn desk_tabs(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (tab, label) in [
                (super::DeskTab::Recent, "Recent"),
                (super::DeskTab::Favorites, "Favorites"),
                (super::DeskTab::Address, "Address book"),
                (super::DeskTab::Lan, "Discovered"),
            ] {
                let selected = self.desk_tab == tab;
                let text = if selected {
                    RichText::new(label).strong().color(p.text)
                } else {
                    RichText::new(label).color(p.muted)
                };
                let fill = if selected { p.elev } else { egui::Color32::TRANSPARENT };
                if ui.add(egui::Button::new(text).fill(fill).frame(selected)).clicked() {
                    self.desk_tab = tab;
                }
            }
            if self.cfg.account.is_some() && self.desk_tab == super::DeskTab::Address {
                ui.label(widgets::muted(ui, "synced"));
            }
        });
        ui.add_space(8.0);

        let rows = self.rows_for_tab();
        if rows.is_empty() {
            ui.label(widgets::muted(ui, self.empty_hint()));
            if self.desk_tab == super::DeskTab::Address && ui.button("Add the ID above").clicked() {
                self.book_target = Some(normalize_id(&self.connect_id));
                self.book_alias.clear();
            }
            return;
        }

        let mut action: Option<(RowAction, String)> = None;
        for row in rows {
            let id = row.id.clone();
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let family = self.cfg.settings.peers.get(&row.id).map(|n| n.os_family.as_str()).unwrap_or("");
                    let icon = if family.is_empty() { Icon::Monitor } else { super::saved::os_icon(family) };
                    theme::icon_in(ui, icon, 22.0, p.muted);
                    if row.online {
                        theme::icon_in(ui, Icon::Check, 10.0, p.ok);
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&row.title).strong().color(p.text));
                        ui.label(widgets::muted(ui, &row.detail));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Connect").clicked() {
                            action = Some((RowAction::Connect, id.clone()));
                        }
                        let star_tip = if row.favorite { "Remove from favorites" } else { "Add to favorites" };
                        if widgets::icon_button_active(ui, Icon::Star, star_tip, row.favorite).clicked() {
                            action = Some((RowAction::Favorite, id.clone()));
                        }
                        if self.desk_tab == super::DeskTab::Address
                            && widgets::icon_button(ui, Icon::Trash, "Remove").clicked()
                        {
                            action = Some((RowAction::Remove, id.clone()));
                        }
                    });
                })
                .response
                .context_menu(|ui| {
                    if ui.button("Connect").clicked() {
                        action = Some((RowAction::Connect, id.clone()));
                        ui.close();
                    }
                    if ui.button("Transfer file").clicked() {
                        action = Some((RowAction::Files, id.clone()));
                        ui.close();
                    }
                    if ui.button(if row.favorite { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                        action = Some((RowAction::Favorite, id.clone()));
                        ui.close();
                    }
                    if ui.button("Copy ID").clicked() {
                        action = Some((RowAction::CopyId, id.clone()));
                        ui.close();
                    }
                    if ui.button("Forget").clicked() {
                        action = Some((RowAction::Forget, id.clone()));
                        ui.close();
                    }
                    if row.in_book && ui.button("Remove from address book").clicked() {
                        action = Some((RowAction::Remove, id.clone()));
                        ui.close();
                    }
                });
            });
            ui.add_space(8.0);
        }

        if self.desk_tab == super::DeskTab::Address && ui.button("Add the ID above").clicked() {
            self.book_target = Some(normalize_id(&self.connect_id));
            self.book_alias.clear();
        }

        if let Some((kind, id)) = action {
            self.apply_row(kind, &id);
        }
    }

    fn apply_row(&mut self, kind: RowAction, id: &str) {
        match kind {
            RowAction::Connect => self.connect_to(id, None, false),
            RowAction::Files => self.connect_to(id, None, true),
            RowAction::Favorite => self.toggle_favorite(id),
            RowAction::Forget => {
                self.cfg.settings.peers.remove(id);
                self.save_cfg();
                self.toast.show("Forgotten");
            }
            RowAction::Remove => {
                self.cfg.address_book.retain(|e| e.id != id);
                self.cfg.remember_password(id, None);
                self.sync_address_book();
                self.toast.show("Removed");
            }
            RowAction::CopyId => {
                if widgets::copy_to_clipboard(&normalize_id(id)) {
                    self.toast.show("ID copied");
                }
            }
        }
    }

    fn rows_for_tab(&self) -> Vec<DeskRow> {
        match self.desk_tab {
            super::DeskTab::Recent => {
                let mut ids = self.recent_ids();
                ids.truncate(12);
                ids.into_iter().map(|id| self.desk_row(&id, false)).collect()
            }
            super::DeskTab::Favorites => self
                .cfg
                .settings
                .peers
                .iter()
                .filter(|(_, n)| n.favorite)
                .map(|(id, _)| id.clone())
                .map(|id| self.desk_row(&id, false))
                .collect(),
            super::DeskTab::Address => self.cfg.address_book.iter().map(|e| self.desk_row(&e.id, false)).collect(),
            super::DeskTab::Lan => self
                .lan
                .iter()
                .map(|(name, id, addr)| DeskRow {
                    id: id.clone(),
                    title: name.clone(),
                    detail: format!("{} · {addr}", format_id(id)),
                    favorite: self.cfg.settings.peers.get(id).is_some_and(|n| n.favorite),
                    online: true,
                    in_book: self.cfg.address_book.iter().any(|e| e.id == *id),
                })
                .collect(),
        }
    }

    fn desk_row(&self, id: &str, online: bool) -> DeskRow {
        let title = self.row_title(id);
        let mut bits = Vec::new();
        if title != format_id(id) {
            bits.push(format_id(id));
        }
        if let Some(note) = self.cfg.settings.peers.get(id) {
            if note.last_connected > 0 {
                bits.push(ago(note.last_connected));
            }
        }
        if let Some(note) = self.cfg.settings.peers.get(id) {
            if !note.os_family.is_empty() {
                bits.push(super::saved::os_label(&note.os_family).to_string());
            }
        }
        if self.saved_for(id).is_some() {
            bits.push("Password saved".into());
        }
        if bits.is_empty() {
            bits.push(format_id(id));
        }
        DeskRow {
            id: id.to_string(),
            title,
            detail: bits.join(" · "),
            favorite: self.cfg.settings.peers.get(id).is_some_and(|n| n.favorite),
            online,
            in_book: self.cfg.address_book.iter().any(|e| e.id == id),
        }
    }

    pub(super) fn row_title(&self, id: &str) -> String {
        self.cfg
            .address_book
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.alias.clone())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| format_id(id))
    }

    fn empty_hint(&self) -> &'static str {
        match self.desk_tab {
            super::DeskTab::Recent => "Computers you connect to show up here.",
            super::DeskTab::Favorites => "Star a computer to keep it here.",
            super::DeskTab::Address => "No saved computers yet. Connect to one, or add the ID above.",
            super::DeskTab::Lan => "Choose Find on this network to look for PJDesk on this LAN.",
        }
    }

    pub(super) fn add_book_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.book_target.clone() else { return };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Add to saved computers")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .constrain(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label("Name (optional)");
                ui.text_edit_singleline(&mut self.book_alias);
                ui.add_space(8.0);
                ui.label("ID");
                let mut id = format_id(&target);
                ui.add_enabled(false, egui::TextEdit::singleline(&mut id));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Save").fill(theme::paint_ctx(ctx).accent)).clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if !open || cancel {
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
        let p = theme::paint_ctx(ctx);
        egui::Window::new("Sign in to your server")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .constrain(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                if self.cfg.server.is_none() {
                    ui.colored_label(p.warn, "Set your server in Settings first.");
                }
                ui.label("Username");
                ui.text_edit_singleline(&mut self.login_user);
                ui.add_space(8.0);
                ui.label("Password");
                let pw = widgets::password_field(ui, &mut self.login_pass, "Password", &mut self.show_login_pass);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if self.login_busy {
                        widgets::spinner(ui, p.accent);
                    }
                    let clicked = ui
                        .add_enabled(
                            !self.login_busy,
                            egui::Button::new(if self.login_busy { "Signing in…" } else { "Sign in" }).fill(p.accent),
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
        let p = theme::paint_ctx(ctx);
        egui::Window::new("Change your password")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .constrain(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(400.0);
                ui.label(RichText::new(&self.password_notice).color(p.warn));
                ui.add_space(8.0);
                ui.label("New password");
                widgets::password_field(
                    ui,
                    &mut self.change_pass,
                    "At least 12 characters, mixed case, a digit, and a symbol",
                    &mut self.show_change_pass,
                );
                ui.add_space(8.0);
                ui.label("Type it again");
                let mut shown = self.show_change_pass;
                widgets::password_field(ui, &mut self.change_pass2, "Repeat the password", &mut shown);
                self.show_change_pass = shown;
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if self.change_rx.is_some() {
                        widgets::spinner(ui, p.accent);
                    }
                    if ui
                        .add_enabled(self.change_rx.is_none(), egui::Button::new("Change password").fill(p.accent))
                        .clicked()
                    {
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

struct DeskRow {
    id: String,
    title: String,
    detail: String,
    favorite: bool,
    online: bool,
    in_book: bool,
}

pub(super) fn format_typed_id(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).take(9).collect();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn server_host(server: &str) -> String {
    server.split(':').next().filter(|s| !s.is_empty()).unwrap_or(server).to_string()
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let secs = now.saturating_sub(unix);
    if secs < 60 {
        "Just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text);
    if line.chars().count() > 72 {
        let truncated: String = line.chars().take(71).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}
