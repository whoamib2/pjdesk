//! Saved computers: one list for everything this app has connected to or been
//! given an ID for, with search, sort, and a favorites filter.

use eframe::egui::{self, RichText, Vec2};
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
                        &format!("Signed in as {email}. This list syncs with your account. Passwords stay on this computer."),
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
            egui::ScrollArea::both().id_salt("saved-rows").auto_shrink([false, false]).show(ui, |ui| {
                let mut action: Option<RowCmd> = None;
                egui::Grid::new("saved-table").num_columns(11).spacing(egui::vec2(6.0, 4.0)).min_col_width(0.0).show(
                    ui,
                    |ui| {
                        header(ui);
                        ui.end_row();
                        for row in &rows {
                            self.row_line(ui, row, &mut action);
                            ui.end_row();
                        }
                    },
                );
                if let Some(cmd) = action {
                    self.apply_saved(cmd);
                }
            });
        });

        if let Some(id) = self.confirm_remove.clone() {
            self.remove_dialog(ui, &id);
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
            if !pjdesk_proto::is_valid_id(&id) || self.cfg.is_saved_hidden(&id) {
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
    /// Signed-in computers upload it. Passwords stay on this computer.
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

    fn row_line(&mut self, ui: &mut egui::Ui, row: &SavedRow, action: &mut Option<RowCmd>) {
        let p = theme::paint(ui);
        let id = row.id.clone();
        icon_cell(ui, Icon::Monitor, p.muted);
        if self.renaming.as_deref() == Some(id.as_str()) {
            let commit = name_editor(ui, &mut self.rename_text);
            if commit {
                *action = Some(RowCmd::CommitRename(id.clone()));
            }
        } else {
            name_cell(ui, &row.nickname, &row.display_name, &row.hostname, p.text, p.muted);
        }
        fixed(ui, W_ID, |ui| {
            ui.add(egui::Label::new(RichText::new(format_id(&id)).monospace().color(p.muted)).truncate());
        });
        fixed(ui, W_COPY, |ui| {
            if widgets::icon_button(ui, Icon::Copy, "Copy ID").clicked() {
                ui.ctx().copy_text(format_id(&id));
                self.toast.show("Copied");
            }
        });
        let user = if row.console_user.trim().is_empty() { "—".to_string() } else { row.console_user.clone() };
        fixed(ui, W_USER, |ui| {
            ui.add(egui::Label::new(RichText::new(user).color(p.text).size(12.0)).truncate());
        });
        let when = if row.last_connected == 0 { "Never".to_string() } else { ago(row.last_connected) };
        fixed(ui, W_LAST, |ui| {
            ui.add(egui::Label::new(RichText::new(when).color(p.muted).size(12.0)).truncate());
        });
        system_cell(ui, &row.os_family, &row.os_version);
        fixed(ui, W_LOCK, |ui| {
            if row.has_password {
                theme::icon_in(ui, Icon::Lock, 14.0, p.muted);
            }
        });
        fixed(ui, W_STATUS, |ui| {
            if row.this_computer {
                ui.add(egui::Label::new(RichText::new("This computer").color(p.accent).size(12.0)).truncate());
            } else if row.online {
                widgets::status_dot(ui, p.ok, "Online");
            } else if row.last_seen == 0 {
                ui.add(egui::Label::new(RichText::new("Offline").color(p.muted).size(12.0)).truncate());
            } else {
                let text = format!("Offline · {}", ago(row.last_seen));
                ui.add(egui::Label::new(RichText::new(text).color(p.muted).size(12.0)).truncate());
            }
        });
        fixed(ui, W_CONNECT, |ui| {
            if row.this_computer {
                ui.add_enabled(false, egui::Button::new("Connect"));
            } else if ui.button("Connect").clicked() {
                *action = Some(RowCmd::Connect(id.clone()));
            }
        });
        fixed(ui, W_MENU, |ui| {
            ui.menu_button("Menu", |ui| {
                if ui.button("Rename").clicked() {
                    *action = Some(RowCmd::Rename(id.clone()));
                    ui.close();
                }
                if !row.in_book && ui.button("Save").clicked() {
                    *action = Some(RowCmd::Save(id.clone()));
                    ui.close();
                }
                let fav = if row.favorite { "Unfavorite" } else { "Favorite" };
                if ui.button(fav).clicked() {
                    *action = Some(RowCmd::Favorite(id.clone()));
                    ui.close();
                }
                if ui.button("Move up").clicked() {
                    *action = Some(RowCmd::Move(id.clone(), true));
                    ui.close();
                }
                if ui.button("Move down").clicked() {
                    *action = Some(RowCmd::Move(id.clone(), false));
                    ui.close();
                }
                if row.has_password && ui.button("Forget saved password").clicked() {
                    *action = Some(RowCmd::ForgetPassword(id.clone()));
                    ui.close();
                }
                if ui.button("Remove").clicked() {
                    *action = Some(RowCmd::Remove(id.clone()));
                    ui.close();
                }
            });
        });
    }

    fn apply_saved(&mut self, cmd: RowCmd) {
        match cmd {
            RowCmd::Connect(id) => self.connect_to(&id, None, false),
            RowCmd::Files(id) => self.connect_to(&id, None, false),
            RowCmd::Save(id) => self.save_computer(&id),
            RowCmd::Favorite(id) => self.toggle_favorite(&id),
            RowCmd::ForgetPassword(id) => {
                self.cfg.remember_password(&id, None);
                if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == id) {
                    entry.password = None;
                }
                self.sync_address_book();
                self.toast.show("Saved password forgotten");
            }
            RowCmd::Remove(id) => self.confirm_remove = Some(id),
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

    fn remove_dialog(&mut self, ui: &mut egui::Ui, id: &str) {
        let shown = format_id(id);
        let mut close = false;
        let mut remove = false;
        egui::Window::new("Remove computer")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(format!("Remove {shown} from saved computers?"));
                ui.label(widgets::muted(ui, "This forgets the name, the saved password, and the last connection. It does not change the other computer."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if widgets::accent_button(ui, "Remove", Vec2::new(100.0, 32.0)) {
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
            self.sync_address_book();
            self.toast.show("Removed");
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
                    ui.label("That password was rejected. Enter the current one for this computer.");
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
    Files(String),
    Save(String),
    Favorite(String),
    ForgetPassword(String),
    Remove(String),
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

const ROW_H: f32 = 46.0;
const W_ICON: f32 = 28.0;
const W_NAME: f32 = 168.0;
const W_ID: f32 = 118.0;
const W_COPY: f32 = 32.0;
const W_USER: f32 = 128.0;
const W_LAST: f32 = 100.0;
const W_SYS: f32 = 132.0;
const W_LOCK: f32 = 18.0;
const W_STATUS: f32 = 132.0;
const W_CONNECT: f32 = 88.0;
const W_MENU: f32 = 76.0;

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

/// One grid cell. The width is fixed so a badge on one row cannot move the buttons on the others.
fn fixed(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(width, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        ui.set_max_width(width);
        add(ui);
    });
}

fn icon_cell(ui: &mut egui::Ui, icon: Icon, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(W_ICON, ROW_H), egui::Sense::hover());
    let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(22.0));
    super::icons::paint(ui.painter(), icon_rect, icon, color);
}

fn name_cell(
    ui: &mut egui::Ui,
    nickname: &str,
    display_name: &str,
    hostname: &str,
    text: egui::Color32,
    muted: egui::Color32,
) {
    let primary = if !nickname.is_empty() {
        nickname.to_string()
    } else if !display_name.is_empty() {
        display_name.to_string()
    } else if !hostname.is_empty() {
        hostname.to_string()
    } else {
        "No name yet".into()
    };
    let secondary = if !hostname.is_empty() && hostname != primary { hostname.to_string() } else { String::new() };
    let placeholder = primary == "No name yet";
    let layout = if secondary.is_empty() {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::top_down(egui::Align::Min)
    };
    ui.allocate_ui_with_layout(Vec2::new(W_NAME, ROW_H), layout, |ui| {
        ui.set_min_width(W_NAME);
        ui.set_max_width(W_NAME);
        let color = if placeholder { muted } else { text };
        ui.add(egui::Label::new(RichText::new(primary).color(color)).truncate());
        if !secondary.is_empty() {
            ui.add(egui::Label::new(RichText::new(secondary).color(muted).size(11.0)).truncate());
        }
    });
}

fn name_editor(ui: &mut egui::Ui, text: &mut String) -> bool {
    let mut commit = false;
    ui.allocate_ui_with_layout(Vec2::new(W_NAME, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(W_NAME);
        ui.set_max_width(W_NAME);
        let edit = ui.add(egui::TextEdit::singleline(text).desired_width(W_NAME - 4.0).hint_text("Nickname"));
        if !edit.has_focus() {
            edit.request_focus();
        }
        if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            commit = true;
        }
    });
    commit
}

fn header_label(ui: &mut egui::Ui, width: f32, text: &str) {
    let p = theme::paint(ui);
    fixed(ui, width, |ui| {
        if !text.is_empty() {
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.muted).strong()).truncate());
        }
    });
}

fn system_cell(ui: &mut egui::Ui, family: &str, version: &str) {
    let p = theme::paint(ui);
    let os = system_label(family, version);
    fixed(ui, W_SYS, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
        let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(16.0));
        super::icons::paint(ui.painter(), icon_rect, os_icon(family), p.text);
        ui.add(egui::Label::new(RichText::new(os).color(p.muted).size(12.0)).truncate());
    });
}

fn header(ui: &mut egui::Ui) {
    let _ = ui.allocate_exact_size(Vec2::new(W_ICON, 16.0), egui::Sense::hover());
    header_label(ui, W_NAME, "Name");
    header_label(ui, W_ID, "ID");
    header_label(ui, W_COPY, "");
    header_label(ui, W_USER, "User");
    header_label(ui, W_LAST, "Last connected");
    header_label(ui, W_SYS, "System");
    header_label(ui, W_LOCK, "");
    header_label(ui, W_STATUS, "");
    header_label(ui, W_CONNECT, "");
    header_label(ui, W_MENU, "");
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
}
