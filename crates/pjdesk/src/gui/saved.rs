//! Saved computers: one list for everything this app has connected to or been
//! given an ID for, with search, sort, and a favorites filter.

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use pjdesk_proto::{format_id, normalize_id};

use super::icons::Icon;
use super::{theme, widgets, App};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SavedSort {
    Name,
    Recent,
    Os,
    Order,
}

pub struct SavedRow {
    pub id: String,
    /// User-typed name. Empty when they have not set one.
    pub nickname: String,
    /// Hostname the host reported. Empty before the first time it was seen.
    pub hostname: String,
    pub os_family: String,
    pub os_version: String,
    pub last_connected: u64,
    pub favorite: bool,
    pub has_password: bool,
    pub online: bool,
    /// Last person signed in at that computer. Empty when none has been reported.
    pub console_user: String,
    /// When the account's server last saw it. 0 when unknown.
    pub last_seen: u64,
    /// This row is the computer the window is running on.
    pub this_computer: bool,
    pub sort_order: u32,
    /// Name chosen on that computer. A nickname typed here replaces it on this row only.
    pub display_name: String,
    pub in_book: bool,
}

pub fn os_icon(family: &str) -> Icon {
    match family {
        "windows" => Icon::OsWindows,
        "macos" => Icon::OsMac,
        "linux" => Icon::OsLinux,
        "android" => Icon::OsAndroid,
        _ => Icon::OsUnknown,
    }
}

pub fn os_label(family: &str) -> &'static str {
    match family {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "android" => "Android",
        _ => "Unknown",
    }
}

/// True when `query` matches the nickname, the hostname, the user, the ID (spaces ignored), or the OS.
pub fn query_matches(query: &str, nickname: &str, hostname: &str, id: &str, os_family: &str, user: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    let q_id: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let id_compact: String = id.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    nickname.to_ascii_lowercase().contains(&q)
        || hostname.to_ascii_lowercase().contains(&q)
        || (!q_id.is_empty() && id_compact.contains(&q_id))
        || os_label(os_family).to_ascii_lowercase().contains(&q)
        || os_family.to_ascii_lowercase().contains(&q)
        || user.to_ascii_lowercase().contains(&q)
}

fn sort_label(row: &SavedRow) -> String {
    if !row.nickname.is_empty() {
        row.nickname.to_ascii_lowercase()
    } else {
        row.hostname.to_ascii_lowercase()
    }
}

pub fn sort_rows(rows: &mut [SavedRow], sort: SavedSort) {
    match sort {
        SavedSort::Name => rows.sort_by(|a, b| sort_label(a).cmp(&sort_label(b)).then(a.id.cmp(&b.id))),
        SavedSort::Recent => {
            rows.sort_by(|a, b| b.last_connected.cmp(&a.last_connected).then(sort_label(a).cmp(&sort_label(b))))
        }
        SavedSort::Os => rows
            .sort_by(|a, b| os_label(&a.os_family).cmp(os_label(&b.os_family)).then(sort_label(a).cmp(&sort_label(b)))),
        SavedSort::Order => rows.sort_by(|a, b| {
            let ao = if a.sort_order == 0 { u32::MAX } else { a.sort_order };
            let bo = if b.sort_order == 0 { u32::MAX } else { b.sort_order };
            ao.cmp(&bo).then(sort_label(a).cmp(&sort_label(b))).then(a.id.cmp(&b.id))
        }),
    }
}

impl App {
    pub(super) fn computer_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let mut submitted = false;
        theme::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, label) in [
                    (super::DeskTab::Saved, "Saved"),
                    (super::DeskTab::Recent, "Recent"),
                    (super::DeskTab::Lan, "On this network"),
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
            });
            ui.add_space(6.0);
            if self.desk_tab == super::DeskTab::Saved {
                if let Some(email) = self.cfg.network_email.as_deref().filter(|s| !s.is_empty()) {
                    ui.label(widgets::muted(
                        ui,
                        &format!("Signed in as {email}. This list syncs with your account. Saved passwords sync too, encrypted so the server cannot read them."),
                    ));
                } else {
                    ui.label(widgets::muted(ui, "Sign in at pjdesk.com to sync across your computers."));
                }
            } else if self.desk_tab == super::DeskTab::Recent {
                ui.label(widgets::muted(ui, "Computers you have connected to on this computer."));
            } else {
                ui.label(widgets::muted(ui, "PJDesk computers that answered on this network."));
            }
            ui.add_space(6.0);
            let search = widgets::text_field(ui, &mut self.saved_query, "Search nickname, name, ID, or OS", false);
            submitted = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::muted(ui, "Sort"));
                egui::ComboBox::from_id_salt("saved-sort")
                    .selected_text(match self.saved_sort {
                        SavedSort::Name => "Name",
                        SavedSort::Recent => "Last connected",
                        SavedSort::Os => "Operating system",
                        SavedSort::Order => "Saved order",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Name, "Name");
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Recent, "Last connected");
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Os, "Operating system");
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Order, "Saved order");
                    });
                widgets::checkbox(ui, &mut self.saved_favorites, "Favorites");
            });
        });
        ui.add_space(8.0);

        let mut rows = match self.desk_tab {
            super::DeskTab::Recent => {
                let mut rows = self.saved_rows();
                rows.retain(|r| r.last_connected > 0 && !r.this_computer);
                rows
            }
            super::DeskTab::Lan => self.lan_rows(),
            super::DeskTab::Saved => self.saved_rows(),
        };
        if self.saved_favorites {
            rows.retain(|r| r.favorite);
        }
        let query = self.saved_query.clone();
        rows.retain(|r| query_matches(&query, &r.nickname, &r.hostname, &r.id, &r.os_family, &r.console_user));
        sort_rows(&mut rows, self.saved_sort);
        if submitted {
            if let Some(first) = rows.iter().find(|r| !r.this_computer) {
                let id = first.id.clone();
                self.connect_to(&id, None, false);
            }
        }

        theme::card(ui, |ui| {
            if rows.is_empty() {
                ui.label(widgets::muted(ui, "No saved computers match."));
                return;
            }
            egui::ScrollArea::vertical().id_salt("saved-rows").auto_shrink([false, false]).show(ui, |ui| {
                let available = ui.available_width();
                let width = if available.is_finite() { (available - ROW_GUTTER).max(0.0) } else { 640.0 };
                let layout = saved_layout(width);
                let mut action: Option<RowCmd> = None;
                saved_header(ui, width, &layout);
                for row in &rows {
                    self.saved_row(ui, row, width, &layout, &mut action);
                }
                if let Some(cmd) = action {
                    self.apply_saved(cmd);
                }
            });
        });

        if let Some(id) = self.confirm_remove.clone() {
            self.remove_dialog(ui, &id);
        }
        if let Some(id) = self.confirm_delete.clone() {
            self.delete_dialog(ui, &id);
        }
    }

    fn saved_rows(&self) -> Vec<SavedRow> {
        use std::collections::BTreeSet;
        let mut ids = BTreeSet::new();
        for entry in &self.cfg.address_book {
            ids.insert(entry.id.clone());
        }
        for id in self.cfg.settings.peers.keys() {
            ids.insert(id.clone());
        }
        let mut rows = Vec::new();
        for id in ids {
            if !pjdesk_proto::is_valid_id(&id) || self.cfg.is_saved_hidden(&id) || self.cfg.account_removed.iter().any(|h| h == &id)
            {
                continue;
            }
            let entry = self.cfg.address_book.iter().find(|e| e.id == id);
            let note = self.cfg.settings.peers.get(&id);
            let hostname = note.map(|n| n.hostname.trim().to_string()).unwrap_or_default();
            let alias = entry.map(|e| e.alias.clone()).unwrap_or_default();
            let nickname = crate::config::nickname_text(&alias, &id, &hostname);
            let online = self.lan.iter().any(|(_, lan_id, _)| normalize_id(lan_id) == id)
                || note.is_some_and(|n| n.reported_online);
            rows.push(SavedRow {
                id: id.clone(),
                nickname,
                hostname,
                os_family: note.map(|n| n.os_family.clone()).unwrap_or_default(),
                os_version: note.map(|n| n.os_version.clone()).unwrap_or_default(),
                last_connected: note.map(|n| n.last_connected).unwrap_or(0),
                favorite: note.map(|n| n.favorite).unwrap_or(false),
                has_password: self.cfg.saved_password(&id).is_some(),
                online,
                console_user: note.map(|n| n.console_user.clone()).unwrap_or_default(),
                last_seen: note.map(|n| n.last_seen).unwrap_or(0),
                this_computer: self.is_this_computer(&id),
                sort_order: note.map(|n| n.sort_order).unwrap_or(0),
                display_name: note.map(|n| n.display_name.clone()).unwrap_or_default(),
                in_book: entry.is_some(),
            });
        }
        rows
    }

    fn lan_rows(&self) -> Vec<SavedRow> {
        let mut rows = Vec::new();
        for (name, id, _addr) in &self.lan {
            let id = normalize_id(id);
            if !pjdesk_proto::is_valid_id(&id) {
                continue;
            }
            let entry = self.cfg.address_book.iter().find(|e| e.id == id);
            let note = self.cfg.settings.peers.get(&id);
            let hostname = note
                .map(|n| n.hostname.trim().to_string())
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| name.trim().to_string());
            let alias = entry.map(|e| e.alias.clone()).unwrap_or_default();
            rows.push(SavedRow {
                id: id.clone(),
                nickname: crate::config::nickname_text(&alias, &id, &hostname),
                hostname,
                os_family: note.map(|n| n.os_family.clone()).unwrap_or_default(),
                os_version: note.map(|n| n.os_version.clone()).unwrap_or_default(),
                last_connected: note.map(|n| n.last_connected).unwrap_or(0),
                favorite: note.map(|n| n.favorite).unwrap_or(false),
                has_password: self.cfg.saved_password(&id).is_some(),
                online: true,
                console_user: note.map(|n| n.console_user.clone()).unwrap_or_default(),
                last_seen: note.map(|n| n.last_seen).unwrap_or(0),
                this_computer: self.is_this_computer(&id),
                sort_order: note.map(|n| n.sort_order).unwrap_or(0),
                display_name: note.map(|n| n.display_name.clone()).unwrap_or_default(),
                in_book: entry.is_some() && !self.cfg.is_saved_hidden(&id),
            });
        }
        rows
    }

    /// Adds a computer from Recent or On this network to the saved list.
    /// Signed-in computers upload it. Passwords travel in the sealed vault, not in this list.
    fn save_computer(&mut self, id: &str) {
        let id = normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        self.cfg.saved_hidden.retain(|h| h != &id);
        if !self.cfg.address_book.iter().any(|e| e.id == id) {
            self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry { id: id.clone(), alias: String::new(), ..Default::default() });
        }
        self.desk_tab = super::DeskTab::Saved;
        self.sync_address_book();
        self.toast.show("Saved");
    }

    fn is_this_computer(&self, id: &str) -> bool {
        let id = normalize_id(id);
        let same = |other: &str| normalize_id(other) == id;
        if self.cfg.device_id.as_deref().is_some_and(same) {
            return true;
        }
        if self.svc_status.id.as_deref().is_some_and(same) {
            return true;
        }
        self.engine.shared.id.lock().unwrap().as_deref().is_some_and(same)
    }

    fn saved_row(&mut self, ui: &mut egui::Ui, row: &SavedRow, width: f32, layout: &SavedLayout, action: &mut Option<RowCmd>) {
        let p = theme::paint(ui);
        let (rect, row_resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
        if row_resp.hovered() {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(8), p.text.linear_multiply(0.06));
        }
        let id = row.id.clone();
        if row_resp.double_clicked() && self.renaming.as_deref() != Some(id.as_str()) {
            if let Some(kind) = primary_action(row.this_computer, &row.os_family) {
                *action = Some(match kind {
                    PrimaryAction::Connect => RowCmd::Connect(id.clone()),
                    PrimaryAction::View => RowCmd::View(id.clone()),
                });
            }
        }
        let slots = column_slots(rect, layout);
        in_rect(ui, slots.icon, (&id, "icon"), |ui| {
            paint_icon(ui, ui.max_rect(), Icon::Monitor, p.muted);
        });
        let renaming = self.renaming.as_deref() == Some(id.as_str());
        if renaming {
            let commit = name_editor(ui, slots.name, &id, &mut self.rename_text);
            if commit {
                *action = Some(RowCmd::CommitRename(id.clone()));
            }
        } else {
            let copied = name_block(ui, slots.name, row, layout, p.text, p.muted);
            if copied {
                ui.ctx().copy_text(format_id(&id));
                self.toast.show("Copied");
            }
        }
        if let Some(r) = slots.id {
            in_rect(ui, r, (&id, "id"), |ui| {
                ui.add(egui::Label::new(RichText::new(format_id(&id)).monospace().color(p.muted).size(13.0)).truncate());
            });
        }
        if let Some(r) = slots.copy {
            in_rect(ui, r, (&id, "copy"), |ui| {
                if widgets::icon_button(ui, Icon::Copy, "Copy ID").clicked() {
                    ui.ctx().copy_text(format_id(&id));
                    self.toast.show("Copied");
                }
            });
        }
        if let Some(r) = slots.user {
            let user = if row.console_user.trim().is_empty() { "—".to_string() } else { row.console_user.clone() };
            in_rect(ui, r, (&id, "user"), |ui| {
                ui.add(egui::Label::new(RichText::new(user).color(p.text).size(12.5)).truncate());
            });
        }
        if let Some(r) = slots.last {
            let when = if row.last_connected == 0 { "Never".to_string() } else { ago(row.last_connected) };
            in_rect(ui, r, (&id, "last"), |ui| {
                ui.add(egui::Label::new(RichText::new(when).color(p.muted).size(12.5)).truncate());
            });
        }
        in_rect(ui, slots.os, (&id, "os"), |ui| {
            paint_icon(ui, ui.max_rect(), os_icon(&row.os_family), p.text);
        });
        if let Some(r) = slots.system_text {
            let os = system_label(&row.os_family, &row.os_version);
            in_rect(ui, r, (&id, "sys"), |ui| {
                ui.add(egui::Label::new(RichText::new(os).color(p.muted).size(12.5)).truncate());
            });
        }
        in_rect(ui, slots.lock, (&id, "lock"), |ui| {
            if row.has_password {
                paint_icon(ui, ui.max_rect(), Icon::Lock, p.muted);
            }
        });
        if let Some(r) = slots.status {
            let text = status_text(row);
            let color = status_color(ui, row);
            in_rect(ui, r, (&id, "status"), |ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, color);
                ui.add(egui::Label::new(RichText::new(text).color(p.text).size(12.5)).truncate());
            });
        }
        self.row_actions(ui, row, &id, slots.connect, slots.menu, layout.compact_connect, action);
    }

    fn row_actions(
        &mut self,
        ui: &mut egui::Ui,
        row: &SavedRow,
        id: &str,
        connect: Rect,
        menu: Rect,
        compact: bool,
        action: &mut Option<RowCmd>,
    ) {
        let p = theme::paint(ui);
        let android = row.os_family.eq_ignore_ascii_case("android");
        in_rect(ui, connect, (id, "connect"), |ui| {
            ui.set_width(connect.width());
            ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if compact {
                    if row.this_computer {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(W_CONNECT_ICON), Sense::hover());
                        super::icons::paint(ui.painter(), rect.shrink(6.0), Icon::Go, p.muted);
                        resp.on_hover_text("This computer");
                    } else {
                        let tip = if android { "View phone" } else { "Connect" };
                        let resp = widgets::icon_button(ui, Icon::Go, tip);
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
                        if resp.clicked() {
                            *action = Some(if android { RowCmd::View(id.to_string()) } else { RowCmd::Connect(id.to_string()) });
                        }
                    }
                } else if row.this_computer {
                    ui.add_enabled(false, egui::Button::new("Connect"));
                } else if android {
                    if ui.button("View phone").clicked() {
                        *action = Some(RowCmd::View(id.to_string()));
                    }
                } else if ui.button("Connect").clicked() {
                    *action = Some(RowCmd::Connect(id.to_string()));
                }
            });
        });
        in_rect(ui, menu, (id, "menu"), |ui| {
            ui.set_width(menu.width());
            ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Menu", |ui| {
                    if !row.this_computer && ui.button("Request view access").clicked() {
                        *action = Some(RowCmd::View(id.to_string()));
                        ui.close();
                    }
                    if ui.button("Rename").clicked() {
                        *action = Some(RowCmd::Rename(id.to_string()));
                        ui.close();
                    }
                    if !row.in_book && ui.button("Save").clicked() {
                        *action = Some(RowCmd::Save(id.to_string()));
                        ui.close();
                    }
                    let fav = if row.favorite { "Unfavorite" } else { "Favorite" };
                    if ui.button(fav).clicked() {
                        *action = Some(RowCmd::Favorite(id.to_string()));
                        ui.close();
                    }
                    if ui.button("Move up").clicked() {
                        *action = Some(RowCmd::Move(id.to_string(), true));
                        ui.close();
                    }
                    if ui.button("Move down").clicked() {
                        *action = Some(RowCmd::Move(id.to_string(), false));
                        ui.close();
                    }
                    if row.has_password && ui.button("Forget saved password").clicked() {
                        *action = Some(RowCmd::ForgetPassword(id.to_string()));
                        ui.close();
                    }
                    if ui.button("Remove from Saved").clicked() {
                        *action = Some(RowCmd::Remove(id.to_string()));
                        ui.close();
                    }
                    if row.this_computer {
                        ui.add_enabled(false, egui::Button::new("Delete from my account")).on_disabled_hover_text(
                            "This computer stays on your account while PJDesk is signed in here.",
                        );
                    } else if self.cfg.network_refresh_token().is_empty() {
                        ui.add_enabled(false, egui::Button::new("Delete from my account"))
                            .on_disabled_hover_text("Sign in to delete a computer from your account.");
                    } else if ui.button("Delete from my account").clicked() {
                        *action = Some(RowCmd::DeleteAccount(id.to_string()));
                        ui.close();
                    }
                });
            });
        });
    }

    fn apply_saved(&mut self, cmd: RowCmd) {
        match cmd {
            RowCmd::Connect(id) => self.connect_to(&id, None, false),
            RowCmd::View(id) => self.request_view(&id),
            RowCmd::Files(id) => self.connect_to(&id, None, false),
            RowCmd::Save(id) => self.save_computer(&id),
            RowCmd::Favorite(id) => self.toggle_favorite(&id),
            RowCmd::ForgetPassword(id) => {
                self.cfg.remember_password(&id, None);
                if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == id) {
                    entry.password = None;
                }
                self.sync_address_book();
                self.sync_passwords(true);
                self.toast.show("Saved password forgotten");
            }
            RowCmd::Remove(id) => self.confirm_remove = Some(id),
            RowCmd::DeleteAccount(id) => self.confirm_delete = Some(id),
            RowCmd::Move(id, up) => self.move_saved(&id, up),
            RowCmd::Rename(id) => {
                let hostname = self.cfg.settings.peers.get(&id).map(|n| n.hostname.clone()).unwrap_or_default();
                let alias =
                    self.cfg.address_book.iter().find(|e| e.id == id).map(|e| e.alias.clone()).unwrap_or_default();
                self.rename_text = crate::config::nickname_text(&alias, &id, &hostname);
                self.renaming = Some(id);
            }
            RowCmd::CommitRename(id) => {
                let mut alias = self.rename_text.trim().to_string();
                if crate::config::same_id(&alias, &id) {
                    alias.clear();
                }
                if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == id) {
                    entry.alias = alias;
                } else if !alias.is_empty() {
                    self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
                        id: id.clone(),
                        alias,
                        ..Default::default()
                    });
                }
                self.renaming = None;
                self.sync_address_book();
                self.toast.show("Nickname saved");
            }
        }
    }

    fn computer_label(&self, id: &str) -> String {
        let entry = self.cfg.address_book.iter().find(|e| e.id == id);
        let note = self.cfg.settings.peers.get(id);
        let hostname = note.map(|n| n.hostname.trim().to_string()).unwrap_or_default();
        let alias = entry.map(|e| e.alias.clone()).unwrap_or_default();
        let nickname = crate::config::nickname_text(&alias, id, &hostname);
        if !nickname.is_empty() {
            nickname
        } else if !hostname.is_empty() {
            hostname
        } else {
            format_id(id)
        }
    }

    fn remove_dialog(&mut self, ui: &mut egui::Ui, id: &str) {
        let shown = format_id(id);
        let name = self.computer_label(id);
        let mut close = false;
        let mut remove = false;
        egui::Window::new("Remove from Saved")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(440.0);
                ui.label(format!("Remove {name} ({shown}) from Saved on this computer?"));
                ui.label(widgets::muted(
                    ui,
                    "This only takes it off the Saved list here. It stays on your pjdesk.com account and on your other computers. The password saved on this computer is removed.",
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if widgets::accent_button(ui, "Remove from Saved", Vec2::new(160.0, 32.0)) {
                        remove = true;
                    }
                });
            });
        if close {
            self.confirm_remove = None;
        }
        if remove {
            let id = id.to_string();
            self.cfg.hide_saved(&id);
            self.confirm_remove = None;
            self.saved_sort = SavedSort::Order;
            self.save_cfg();
            self.toast.show("Removed from Saved");
        }
    }

    fn delete_dialog(&mut self, ui: &mut egui::Ui, id: &str) {
        let shown = format_id(id);
        let name = self.computer_label(id);
        let mut close = false;
        let mut delete = false;
        egui::Window::new("Delete from my account")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(440.0);
                ui.label(format!("Delete {name} ({shown}) from your PJDesk account?"));
                ui.label(widgets::muted(
                    ui,
                    "It leaves Saved on every computer signed in to this account. The password saved on this computer is removed. If that computer is signed in, it is signed out. Signing in on it again adds it back.",
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if widgets::danger_button(ui, "Delete from my account", Vec2::new(210.0, 32.0)) {
                        delete = true;
                    }
                });
            });
        if close {
            self.confirm_delete = None;
        }
        if delete {
            let id = id.to_string();
            self.confirm_delete = None;
            self.start_account_delete(id);
        }
    }

    /// Rewrites Saved order so `id` swaps with its neighbor. The new order syncs
    /// only while this computer is signed in to pjdesk.com.
    fn move_saved(&mut self, id: &str, up: bool) {
        let mut rows = self.saved_rows();
        sort_rows(&mut rows, SavedSort::Order);
        let Some(index) = rows.iter().position(|r| r.id == id) else { return };
        let other = if up { index.checked_sub(1) } else { (index + 1 < rows.len()).then_some(index + 1) };
        let Some(other) = other else { return };
        rows.swap(index, other);
        for (place, row) in rows.iter().enumerate() {
            self.cfg.settings.peers.entry(row.id.clone()).or_default().sort_order = ((place as u32) + 1) * 10;
        }
        self.saved_sort = SavedSort::Order;
        self.sync_address_book();
        self.toast.show("Order saved");
    }

    pub(super) fn password_prompt(&mut self, ctx: &egui::Context) {
        if !self.prompt_open {
            return;
        }
        let mut close = false;
        let mut go = false;
        let mut ask_person = false;
        let name = if self.prompt_direct.is_some() {
            self.prompt_id.clone()
        } else {
            self.row_title(&self.prompt_id)
        };
        let title = format!("Password for {name}");
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let id_line = if self.prompt_direct.is_some() { self.prompt_id.clone() } else { format_id(&self.prompt_id) };
                ui.label(RichText::new(id_line).monospace().color(theme::paint(ui).muted));
                if self.prompt_rejected {
                    ui.add_space(4.0);
                    let line = if self.status.starts_with("Incorrect password") {
                        self.status.clone()
                    } else {
                        format!("Incorrect password for {}.", format_id(&self.prompt_id))
                    };
                    ui.label(RichText::new(line).color(theme::paint(ui).warn));
                }
                if self.prompt_checking {
                    ui.add_space(4.0);
                    ui.label("Checking the password…");
                }
                ui.add_space(8.0);
                let label = format!("Password for {name}");
                widgets::field_label(ui, &label);
                let resp = widgets::password_field(ui, &mut self.prompt_password, "Computer password", &mut self.prompt_show);
                ui.add_space(4.0);
                widgets::checkbox(ui, &mut self.prompt_remember, "Save password for this computer");
                ui.label(widgets::muted(
                    ui,
                    "The password on this computer, not your PJDesk account. Saved on this computer only, and not uploaded.",
                ));
                ui.add_space(8.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if !self.prompt_checking && ui.button("Ask to connect instead").clicked() {
                        ask_person = true;
                    }
                    if !self.prompt_checking && (widgets::accent_button(ui, "Connect", Vec2::new(120.0, 36.0)) || enter) {
                        go = true;
                    }
                });
            });
        if close {
            self.write_ask_reply("cancel\n");
            self.prompt_open = false;
            self.prompt_checking = false;
            self.ask_alive = false;
            self.ask_path = None;
        }
        if ask_person {
            if self.ask_alive {
                self.write_ask_reply("ask\n");
                self.prompt_checking = true;
                self.prompt_rejected = false;
            } else {
                let (id, files) = (self.prompt_id.clone(), self.prompt_files);
                self.prompt_open = false;
                self.prompt_checking = false;
                self.ask_person_next = true;
                self.spawn_viewer(&id, "", files, false, false);
                // The viewer asks the person after the computer answers, with no password.
            }
        }
        if go {
            let pw = self.prompt_password.clone();
            if pw.trim().is_empty() {
                self.status = "Enter the password.".into();
            } else if self.ask_alive {
                self.remember = self.prompt_remember;
                let body = format!("password\n{}\n", hex::encode(pw.as_bytes()));
                self.write_ask_reply(&body);
                self.prompt_checking = true;
                self.prompt_rejected = false;
                if self.prompt_remember {
                    // The viewer stores it after the computer accepts, when PJDESK_REMEMBER is set.
                }
            } else {
                let (id, files, remember) = (self.prompt_id.clone(), self.prompt_files, self.prompt_remember);
                self.remember = remember;
                self.prompt_open = false;
                self.prompt_checking = false;
                self.prompt_rejected = false;
                self.ask_alive = false;
                self.ask_path = None;
                if self.prompt_direct.is_some() {
                    self.spawn_direct_with_password(&id, &pw);
                } else {
                    self.spawn_viewer(&id, &pw, files, remember, false);
                }
            }
        }
    }
}

enum RowCmd {
    Connect(String),
    View(String),
    Files(String),
    Save(String),
    Favorite(String),
    ForgetPassword(String),
    Remove(String),
    DeleteAccount(String),
    Rename(String),
    CommitRename(String),
    Move(String, bool),
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let secs = now.saturating_sub(unix);
    if secs < 60 {
        "Just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} hr ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

/// Narrowest window inner width. 720 points is 900 physical pixels at 125% DPI,
/// and that width still shows the name, the status, and Connect / Menu.
pub(crate) const MIN_INNER_WIDTH: f32 = 720.0;

const ROW_H: f32 = 46.0;
const GAP: f32 = 6.0;
/// Room so the floating scrollbar does not cover Menu.
const ROW_GUTTER: f32 = 10.0;
const W_ICON: f32 = 28.0;
const W_OS: f32 = 22.0;
const W_LOCK: f32 = 18.0;
const W_COPY: f32 = 32.0;
const W_CONNECT: f32 = 118.0;
const W_CONNECT_ICON: f32 = 32.0;
const W_MENU: f32 = 76.0;
const NAME_MIN: f32 = 120.0;
const NAME_PREF: f32 = 200.0;
const USER_PREF: f32 = 120.0;
const USER_MIN: f32 = 72.0;
const LAST_PREF: f32 = 104.0;
const LAST_MIN: f32 = 80.0;
const SYS_PREF: f32 = 96.0;
const SYS_MIN: f32 = 72.0;
const ID_PREF: f32 = 118.0;
const ID_MIN: f32 = 108.0;
const STATUS_PREF: f32 = 128.0;
const STATUS_MIN: f32 = 110.0;
/// Below this list width, Connect becomes an icon so the other columns can stay.
const COMPACT_BELOW: f32 = 640.0;

fn system_label(family: &str, version: &str) -> String {
    if family.is_empty() {
        return "Unknown".into();
    }
    if version.is_empty() {
        os_label(family).to_string()
    } else {
        format!("{} {version}", os_label(family))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryAction {
    Connect,
    View,
}

/// What a double-click on the row does. This computer does not connect to itself.
fn primary_action(this_computer: bool, os_family: &str) -> Option<PrimaryAction> {
    if this_computer {
        None
    } else if os_family.eq_ignore_ascii_case("android") {
        Some(PrimaryAction::View)
    } else {
        Some(PrimaryAction::Connect)
    }
}

#[derive(Clone, Copy, Debug)]
struct SavedLayout {
    show_user: bool,
    show_last: bool,
    show_system_text: bool,
    show_id: bool,
    status_in_name: bool,
    compact_connect: bool,
    name: f32,
    user: f32,
    last: f32,
    system_text: f32,
    id: f32,
    status: f32,
    actions: f32,
}

impl SavedLayout {
    fn actions_width(compact: bool) -> f32 {
        let connect = if compact { W_CONNECT_ICON } else { W_CONNECT };
        connect + GAP + W_MENU
    }

    fn used(&self) -> f32 {
        let mut parts = 0.0;
        let mut n = 0.0;
        let mut add = |w: f32| {
            parts += w;
            n += 1.0;
        };
        add(W_ICON);
        add(self.name);
        if self.show_id {
            add(self.id);
            add(W_COPY);
        }
        if self.show_user {
            add(self.user);
        }
        if self.show_last {
            add(self.last);
        }
        add(W_OS);
        if self.show_system_text {
            add(self.system_text);
        }
        add(W_LOCK);
        if !self.status_in_name {
            add(self.status);
        }
        add(self.actions);
        parts + GAP * (n - 1.0)
    }

    fn fits(&self, available: f32) -> bool {
        self.used() <= available + 0.5 && self.name + 0.5 >= NAME_MIN
    }
}

struct Flex {
    pref: f32,
    min: f32,
}

/// Columns for one list width. Actions stay a fixed width on the right.
/// User, Last connected, the system text, then the ID column drop in that
/// order. The ID then sits under the name. The status dot folds into the
/// name after that. Connect becomes an icon below [`COMPACT_BELOW`].
fn saved_layout(available: f32) -> SavedLayout {
    let available = if available.is_finite() { available.max(0.0) } else { 800.0 };
    let compact = available < COMPACT_BELOW;
    let stages = [
        (true, true, true, true, true),
        (false, true, true, true, true),
        (false, false, true, true, true),
        (false, false, false, true, true),
        (false, false, false, false, true),
        (false, false, false, false, false),
    ];
    for (user, last, sys, id, status_beside) in stages {
        if let Some(layout) = fit(available, user, last, sys, id, status_beside, compact) {
            return layout;
        }
    }
    fit(available, false, false, false, false, false, true).unwrap_or(SavedLayout {
        show_user: false,
        show_last: false,
        show_system_text: false,
        show_id: false,
        status_in_name: true,
        compact_connect: true,
        name: 0.0,
        user: 0.0,
        last: 0.0,
        system_text: 0.0,
        id: 0.0,
        status: 0.0,
        actions: SavedLayout::actions_width(true),
    })
}

fn fit(
    available: f32,
    user: bool,
    last: bool,
    sys: bool,
    id: bool,
    status_beside: bool,
    compact: bool,
) -> Option<SavedLayout> {
    let actions = SavedLayout::actions_width(compact);
    let mut fixed = W_ICON + W_OS + W_LOCK + actions;
    let mut n = 5.0;
    if id {
        fixed += W_COPY;
        n += 2.0;
    }
    if user {
        n += 1.0;
    }
    if last {
        n += 1.0;
    }
    if sys {
        n += 1.0;
    }
    if status_beside {
        n += 1.0;
    }
    let budget = available - fixed - GAP * (n - 1.0);
    let mut cols = Vec::new();
    if user {
        cols.push(Flex { pref: USER_PREF, min: USER_MIN });
    }
    if last {
        cols.push(Flex { pref: LAST_PREF, min: LAST_MIN });
    }
    if sys {
        cols.push(Flex { pref: SYS_PREF, min: SYS_MIN });
    }
    if id {
        cols.push(Flex { pref: ID_PREF, min: ID_MIN });
    }
    if status_beside {
        cols.push(Flex { pref: STATUS_PREF, min: STATUS_MIN });
    }
    cols.push(Flex { pref: NAME_PREF, min: NAME_MIN });
    let min_sum: f32 = cols.iter().map(|c| c.min).sum();
    if budget + 0.5 < min_sum {
        return None;
    }
    let mut widths: Vec<f32> = cols.iter().map(|c| c.pref).collect();
    let sum: f32 = widths.iter().sum();
    if sum > budget {
        let mut over = sum - budget;
        for (w, col) in widths.iter_mut().zip(cols.iter()) {
            let cut = (*w - col.min).max(0.0).min(over);
            *w -= cut;
            over -= cut;
        }
    } else {
        let extra = budget - sum;
        if let Some(name) = widths.last_mut() {
            *name += extra;
        }
    }
    let mut next = widths.into_iter();
    let user_w = user.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let last_w = last.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let sys_w = sys.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let id_w = id.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let status_w = status_beside.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let name = next.next().unwrap_or(NAME_MIN);
    Some(SavedLayout {
        show_user: user,
        show_last: last,
        show_system_text: sys,
        show_id: id,
        status_in_name: !status_beside,
        compact_connect: compact,
        name,
        user: user_w,
        last: last_w,
        system_text: sys_w,
        id: id_w,
        status: status_w,
        actions,
    })
}

/// List row width for a window inner size, in points. The card margin and the
/// scrollbar gutter are already removed, which is the width [`saved_layout`] sees.
fn row_width_for_window(window_inner: f32) -> f32 {
    let body = (window_inner - 32.0).max(0.0);
    let list = super::panels::desk_split(body).list;
    (list - 32.0 - ROW_GUTTER).max(0.0)
}

struct Slots {
    icon: Rect,
    name: Rect,
    id: Option<Rect>,
    copy: Option<Rect>,
    user: Option<Rect>,
    last: Option<Rect>,
    os: Rect,
    system_text: Option<Rect>,
    lock: Rect,
    status: Option<Rect>,
    connect: Rect,
    menu: Rect,
}

fn column_slots(row: Rect, layout: &SavedLayout) -> Slots {
    let actions = Rect::from_min_max(Pos2::new((row.right() - layout.actions).max(row.left()), row.top()), row.right_bottom());
    let menu = Rect::from_min_max(Pos2::new(actions.right() - W_MENU, actions.top()), actions.right_bottom());
    let connect = Rect::from_min_max(actions.left_top(), Pos2::new((menu.left() - GAP).max(actions.left()), actions.bottom()));
    let content_right = (actions.left() - GAP).max(row.left());
    let content = Rect::from_min_max(row.left_top(), Pos2::new(content_right, row.bottom()));
    let mut edge = content.right();
    let mut take = |w: f32| -> Rect {
        let rect = Rect::from_min_max(Pos2::new(edge - w, content.top()), Pos2::new(edge, content.bottom()));
        edge -= w + GAP;
        rect
    };
    let status = if layout.status_in_name { None } else { Some(take(layout.status)) };
    let lock = take(W_LOCK);
    let system_text = if layout.show_system_text { Some(take(layout.system_text)) } else { None };
    let os = take(W_OS);
    let last = if layout.show_last { Some(take(layout.last)) } else { None };
    let user = if layout.show_user { Some(take(layout.user)) } else { None };
    let copy = if layout.show_id { Some(take(W_COPY)) } else { None };
    let id = if layout.show_id { Some(take(layout.id)) } else { None };
    let icon = Rect::from_min_size(content.left_top(), Vec2::new(W_ICON, content.height()));
    let name_left = icon.right() + GAP;
    let name = Rect::from_min_max(Pos2::new(name_left, content.top()), Pos2::new(edge.max(name_left), content.bottom()));
    Slots { icon, name, id, copy, user, last, os, system_text, lock, status, connect, menu }
}

fn in_rect(ui: &mut egui::Ui, rect: Rect, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut egui::Ui)) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(salt)
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_clip_rect(rect);
    add(&mut child);
}

fn paint_icon(ui: &egui::Ui, rect: Rect, icon: Icon, color: egui::Color32) {
    let size = 18.0_f32.min(rect.width()).min(rect.height());
    if size < 1.0 {
        return;
    }
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(size));
    super::icons::paint(ui.painter(), icon_rect, icon, color);
}

fn status_text(row: &SavedRow) -> String {
    if row.this_computer {
        "This computer".into()
    } else if row.online {
        "Online".into()
    } else if row.last_seen == 0 {
        "Offline".into()
    } else {
        format!("Offline · {}", ago(row.last_seen))
    }
}

fn status_color(ui: &egui::Ui, row: &SavedRow) -> egui::Color32 {
    let p = theme::paint(ui);
    if row.this_computer {
        p.accent
    } else if row.online {
        p.ok
    } else {
        p.muted
    }
}

fn saved_header(ui: &mut egui::Ui, width: f32, layout: &SavedLayout) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::hover());
    let slots = column_slots(rect, layout);
    let p = theme::paint(ui);
    let label = |ui: &mut egui::Ui, rect: Rect, salt: &str, text: &str| {
        if text.is_empty() {
            return;
        }
        in_rect(ui, rect, ("hdr", salt), |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.muted).strong()).truncate());
        });
    };
    label(ui, slots.name, "name", "Name");
    if let Some(r) = slots.id {
        label(ui, r, "id", "ID");
    }
    if let Some(r) = slots.user {
        label(ui, r, "user", "User");
    }
    if let Some(r) = slots.last {
        label(ui, r, "last", "Last connected");
    }
    if layout.show_system_text {
        let system = Rect::from_min_max(slots.os.left_top(), slots.system_text.unwrap_or(slots.os).right_bottom());
        label(ui, system, "sys", "System");
    }
    if let Some(r) = slots.status {
        label(ui, r, "status", "Status");
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, p.line));
    ui.add_space(4.0);
}

fn name_block(ui: &mut egui::Ui, mut rect: Rect, row: &SavedRow, layout: &SavedLayout, text: egui::Color32, muted: egui::Color32) -> bool {
    let mut copied = false;
    if layout.status_in_name {
        let color = status_color(ui, row);
        let center = Pos2::new(rect.left() + 8.0, rect.center().y);
        let dot = Rect::from_center_size(center, Vec2::splat(12.0));
        ui.painter().circle_filled(center, 4.0, color);
        let hover = ui.interact(dot, ui.id().with((&row.id, "status-dot")), Sense::hover());
        hover.on_hover_text(status_text(row));
        rect = rect.with_min_x(rect.left() + 18.0);
    }
    let show_copy = !layout.show_id && rect.width() >= 150.0;
    if show_copy {
        let copy = Rect::from_min_max(Pos2::new(rect.right() - W_COPY, rect.top()), rect.right_bottom());
        in_rect(ui, copy, (&row.id, "copy-name"), |ui| {
            if widgets::icon_button(ui, Icon::Copy, "Copy ID").clicked() {
                copied = true;
            }
        });
        rect = rect.with_max_x(copy.left());
    }
    let primary = if !row.nickname.is_empty() {
        row.nickname.clone()
    } else if !row.display_name.is_empty() {
        row.display_name.clone()
    } else if !row.hostname.is_empty() {
        row.hostname.clone()
    } else {
        "No name yet".into()
    };
    let mut secondary = if !row.hostname.is_empty() && row.hostname != primary { row.hostname.clone() } else { String::new() };
    if !layout.show_id {
        let id_text = format_id(&row.id);
        secondary = if secondary.is_empty() { id_text } else { format!("{secondary} · {id_text}") };
    }
    let placeholder = primary == "No name yet";
    let stacked = !secondary.is_empty();
    in_rect(ui, rect, (&row.id, "name"), |ui| {
        ui.set_min_width(rect.width().max(0.0));
        ui.set_max_width(rect.width().max(0.0));
        if stacked {
            ui.set_height(rect.height());
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let color = if placeholder { muted } else { text };
                ui.add(egui::Label::new(RichText::new(primary).color(color)).truncate());
                ui.add(egui::Label::new(RichText::new(secondary).color(muted).size(11.0)).truncate());
            });
        } else {
            let color = if placeholder { muted } else { text };
            ui.add(egui::Label::new(RichText::new(primary).color(color)).truncate());
        }
    });
    copied
}

fn name_editor(ui: &mut egui::Ui, rect: Rect, id: &str, text: &mut String) -> bool {
    let mut commit = false;
    in_rect(ui, rect, (id, "rename"), |ui| {
        ui.set_min_width(rect.width().max(0.0));
        ui.set_max_width(rect.width().max(0.0));
        let edit = ui.add(egui::TextEdit::singleline(text).desired_width((rect.width() - 4.0).max(40.0)).hint_text("Nickname"));
        if !edit.has_focus() {
            edit.request_focus();
        }
        if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            commit = true;
        }
    });
    commit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_name_id_with_spaces_and_os() {
        assert!(query_matches("front", "Front desk", "OFFICE-PC", "123456789", "windows", "Ben"));
        assert!(query_matches("office-pc", "Front desk", "OFFICE-PC", "123456789", "windows", ""));
        assert!(query_matches("123 456", "Front desk", "OFFICE-PC", "123456789", "windows", ""));
        assert!(query_matches("linux", "", "BEN-LAPTOP", "555666777", "linux", ""));
        assert!(query_matches("mac", "Studio", "mac-studio", "555666777", "macos", ""));
        assert!(query_matches("spader", "Front desk", "OFFICE-PC", "123456789", "windows", "SPADER\\Ben"));
        assert!(!query_matches("android", "Front desk", "OFFICE-PC", "123456789", "windows", "Ben"));
        assert!(query_matches("   ", "Front desk", "OFFICE-PC", "123456789", "", ""));
        assert_eq!(system_label("", ""), "Unknown");
        assert_eq!(system_label("windows", "11"), "Windows 11");
    }

    #[test]
    fn sort_puts_favorites_recent_first_when_asked() {
        let mut rows = vec![
            SavedRow {
                id: "222222222".into(),
                nickname: String::new(),
                hostname: "Zed".into(),
                os_family: "linux".into(),
                os_version: String::new(),
                last_connected: 10,
                favorite: false,
                has_password: false,
                online: false,
                console_user: String::new(),
                last_seen: 0,
                this_computer: false,
                sort_order: 0,
                display_name: String::new(),
                in_book: true,
            },
            SavedRow {
                id: "111111111".into(),
                nickname: "Alpha".into(),
                hostname: "OFFICE-PC".into(),
                os_family: "windows".into(),
                os_version: String::new(),
                last_connected: 50,
                favorite: true,
                has_password: true,
                online: false,
                console_user: "Ben".into(),
                last_seen: 40,
                this_computer: false,
                sort_order: 0,
                display_name: String::new(),
                in_book: true,
            },
        ];
        sort_rows(&mut rows, SavedSort::Name);
        assert_eq!(rows[0].nickname, "Alpha");
        sort_rows(&mut rows, SavedSort::Recent);
        assert_eq!(rows[0].id, "111111111");
        sort_rows(&mut rows, SavedSort::Os);
        assert_eq!(rows[0].os_family, "linux");
    }

    #[test]
    fn double_click_connects_except_on_this_computer() {
        assert_eq!(primary_action(false, "windows"), Some(PrimaryAction::Connect));
        assert_eq!(primary_action(false, "linux"), Some(PrimaryAction::Connect));
        assert_eq!(primary_action(false, "Android"), Some(PrimaryAction::View));
        assert_eq!(primary_action(true, "android"), None);
        assert_eq!(primary_action(true, "windows"), None);
    }

    #[test]
    fn saved_columns_keep_actions_at_the_tested_widths() {
        // Physical pixels at 100% and 125% DPI. 125% is the same layout with
        // fewer points, which is the width the window actually lays out in.
        let cases = [(900.0, 1.0), (900.0, 1.25), (1024.0, 1.0), (1024.0, 1.25), (1280.0, 1.0), (1280.0, 1.25)];
        for (px, dpi) in cases {
            let points = px / dpi;
            assert!(points + 0.1 >= MIN_INNER_WIDTH, "{px}px at {dpi} is below the window minimum");
            let row = row_width_for_window(points);
            let layout = saved_layout(row);
            assert!(
                layout.fits(row),
                "{px}px at {dpi} (row {row:.1}) does not fit: {layout:?} used {:.1}",
                layout.used()
            );
            assert!(layout.name + 0.5 >= NAME_MIN, "name {:.1} at {px}px/{dpi}", layout.name);
            assert!(layout.status_in_name || layout.status + 0.5 >= STATUS_MIN, "status missing at {px}px/{dpi}: {layout:?}");
            assert_eq!(layout.compact_connect, row < COMPACT_BELOW, "compact at {px}px/{dpi} row {row:.1}");
            let connect = if layout.compact_connect { W_CONNECT_ICON } else { W_CONNECT };
            assert!((layout.actions - (connect + GAP + W_MENU)).abs() < 0.1);
        }
    }

    #[test]
    fn lower_priority_columns_hide_first_and_stay_hidden() {
        let mut prev = saved_layout(1400.0);
        assert!(prev.show_user && prev.show_last && prev.show_system_text && prev.show_id);
        assert!(!prev.status_in_name && !prev.compact_connect);
        let mut dropped_user = None;
        let mut dropped_last = None;
        let mut dropped_sys = None;
        let mut dropped_id = None;
        let mut merged_status = None;
        let mut w = 1400.0;
        while w >= 280.0 {
            let layout = saved_layout(w);
            if layout.fits(w) {
                assert!(layout.name + 0.5 >= NAME_MIN, "name collapsed at {w}: {layout:?}");
            }
            if layout.compact_connect == prev.compact_connect {
                assert!(prev.show_user || !layout.show_user, "user came back at {w}");
                assert!(prev.show_last || !layout.show_last, "last came back at {w}");
                assert!(prev.show_system_text || !layout.show_system_text, "system text came back at {w}");
                assert!(prev.show_id || !layout.show_id, "id column came back at {w}");
                assert!(!prev.status_in_name || layout.status_in_name, "status left the name at {w}");
            }
            if dropped_user.is_none() && !layout.show_user {
                dropped_user = Some(w);
            }
            if dropped_last.is_none() && !layout.show_last {
                dropped_last = Some(w);
            }
            if dropped_sys.is_none() && !layout.show_system_text {
                dropped_sys = Some(w);
            }
            if dropped_id.is_none() && !layout.show_id {
                dropped_id = Some(w);
            }
            if merged_status.is_none() && layout.status_in_name {
                merged_status = Some(w);
            }
            if !layout.show_id {
                assert!(layout.status_in_name || layout.status > 0.0);
            }
            prev = layout;
            w -= 5.0;
        }
        let user = dropped_user.expect("user hides");
        let last = dropped_last.expect("last hides");
        let sys = dropped_sys.expect("system text hides");
        let id = dropped_id.expect("id column hides");
        let status = merged_status.expect("status merges into the name");
        assert!(user > last && last > sys && sys > id && id > status, "hide order user {user} last {last} sys {sys} id {id} status {status}");
        assert!(!saved_layout(COMPACT_BELOW).compact_connect);
        assert!(saved_layout(COMPACT_BELOW - 1.0).compact_connect);
    }
}
