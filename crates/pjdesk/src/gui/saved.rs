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

#[derive(Clone)]
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
    /// This computer joined through a support key. It is listed under Clients.
    pub clients: bool,
    /// Direct LAN address (`host:port`). Set only for a computer found on this network.
    pub direct: Option<String>,
    /// PJDesk version that computer registered. Empty when it has not been reported.
    pub app_version: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SavedView {
    List,
    Small,
    Big,
}

pub fn parse_saved_view(text: &str) -> SavedView {
    match text.trim() {
        "small" => SavedView::Small,
        "big" => SavedView::Big,
        _ => SavedView::List,
    }
}

pub fn saved_view_key(view: SavedView) -> &'static str {
    match view {
        SavedView::List => "list",
        SavedView::Small => "small",
        SavedView::Big => "big",
    }
}

/// Pleasant tile colors. The same id always picks the same one.
const TILE_PALETTE: [egui::Color32; 12] = [
    egui::Color32::from_rgb(0x5c, 0x3a, 0x2e),
    egui::Color32::from_rgb(0x7a, 0x2e, 0x45),
    egui::Color32::from_rgb(0x5b, 0x3a, 0x78),
    egui::Color32::from_rgb(0x1f, 0x4d, 0x3a),
    egui::Color32::from_rgb(0x3d, 0x2f, 0x6b),
    egui::Color32::from_rgb(0x6b, 0x2d, 0x4a),
    egui::Color32::from_rgb(0x1e, 0x4a, 0x55),
    egui::Color32::from_rgb(0x3d, 0x45, 0x4d),
    egui::Color32::from_rgb(0x8a, 0x5a, 0x1e),
    egui::Color32::from_rgb(0x2c, 0x4a, 0x6e),
    egui::Color32::from_rgb(0x2e, 0x4a, 0x32),
    egui::Color32::from_rgb(0x6e, 0x3a, 0x22),
];

/// ID when there is one, otherwise the name. Spaces are ignored.
pub fn tile_key(id: &str, name: &str) -> String {
    let compact: String = id.chars().filter(|c| !c.is_whitespace()).collect();
    if !compact.is_empty() {
        compact.to_ascii_lowercase()
    } else {
        name.trim().to_ascii_lowercase()
    }
}

pub fn tile_index(key: &str) -> usize {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash as usize % TILE_PALETTE.len()
}

pub fn tile_color(key: &str) -> egui::Color32 {
    TILE_PALETTE[tile_index(key)]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTone {
    /// Online, because the server acked this computer or the account says so.
    Ok,
    /// Offline.
    Warn,
}

/// Online from the account's live presence flag, or from a computer heard on
/// this LAN. Last-connected time is a separate column and does not count.
pub fn presence_online(reported_online: bool, on_lan: bool) -> bool {
    on_lan || reported_online
}

pub fn status_tone(online: bool, _this_computer: bool) -> StatusTone {
    if online { StatusTone::Ok } else { StatusTone::Warn }
}

/// This computer's row shows the build that is running. Other rows show the
/// version the account last stored.
pub fn row_version(this_computer: bool, reported: &str) -> String {
    if this_computer {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        reported.trim().to_string()
    }
}

/// One line for the log when the account list is applied.
pub fn presence_refresh_line(rows: &[crate::network::HostPresence]) -> String {
    if rows.is_empty() {
        return "Status refresh from the account: no computers".into();
    }
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let state = if row.online {
                "online".to_string()
            } else if row.last_seen == 0 {
                "offline".to_string()
            } else {
                format!("offline last_seen {}", row.last_seen)
            };
            format!("{} {state}", pjdesk_proto::format_id(&row.id))
        })
        .collect();
    format!("Status refresh from the account: {}", parts.join(", "))
}

/// `user@host` shown on a colored tile. The host is the computer name when no hostname is known.
pub fn tile_caption(user: &str, hostname: &str, display_name: &str, nickname: &str, id: &str) -> String {
    let host = if !hostname.trim().is_empty() {
        hostname.trim().to_string()
    } else if !display_name.trim().is_empty() {
        display_name.trim().to_string()
    } else if !nickname.trim().is_empty() {
        nickname.trim().to_string()
    } else {
        format_id(id)
    };
    let user = user.trim();
    if user.is_empty() {
        host
    } else {
        format!("{user}@{host}")
    }
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

/// Clients first, grouped by the key's label. Everyone else stays in the original order.
pub fn client_sections(rows: Vec<SavedRow>) -> (Vec<(String, Vec<SavedRow>)>, Vec<SavedRow>) {
    let mut groups: Vec<(String, Vec<SavedRow>)> = Vec::new();
    let mut rest = Vec::new();
    for row in rows {
        if row.clients {
            let label = if row.nickname.trim().is_empty() { "Support key".to_string() } else { row.nickname.clone() };
            if let Some((_, list)) = groups.iter_mut().find(|(name, _)| name == &label) {
                list.push(row);
            } else {
                groups.push((label, vec![row]));
            }
        } else {
            rest.push(row);
        }
    }
    (groups, rest)
}

impl App {
    pub(super) fn computer_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let mut submitted = false;
        theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (tab, label) in [
                        (super::DeskTab::Saved, "Saved"),
                        (super::DeskTab::Recent, "Recent"),
                        (super::DeskTab::Lan, "Discovered"),
                    ] {
                        let selected = self.desk_tab == tab;
                        if tab == super::DeskTab::Lan {
                            let color = if selected { p.text } else { p.muted };
                            let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            super::icons::paint(ui.painter(), icon_rect, Icon::Compass, color);
                        }
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
                let current = parse_saved_view(&self.cfg.settings.saved_view);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (view, icon, tip) in [
                        (SavedView::Big, Icon::ViewBig, "Big tiles"),
                        (SavedView::Small, Icon::ViewSmall, "Small tiles"),
                        (SavedView::List, Icon::ViewList, "List"),
                    ] {
                        if widgets::icon_button_active(ui, icon, tip, current == view).clicked() {
                            self.set_saved_view(view);
                        }
                    }
                });
            });
            ui.add_space(6.0);
            if self.desk_tab == super::DeskTab::Saved {
                if self.signed_in() && !self.cfg.syncs_saved_computers() {
                    let email = self.cfg.network_email.as_deref().unwrap_or("").trim();
                    let line = if email.is_empty() {
                        "Signed in. This computer has not loaded your saved computers.".to_string()
                    } else {
                        format!("Signed in as {email}. This computer has not loaded your saved computers.")
                    };
                    ui.label(widgets::muted(ui, &line));
                    if ui.button("Load my saved computers").clicked() {
                        self.load_saved_computers();
                    }
                } else if let Some(email) = self.cfg.network_email.as_deref().filter(|s| !s.is_empty()) {
                    ui.label(widgets::muted(
                        ui,
                        &format!("Signed in as {email}. This list syncs with your account. Saved passwords sync too, encrypted so the server cannot read them."),
                    ));
                } else {
                    ui.label(widgets::muted(ui, "Sign in at pjdesk.com to sync across your computers."));
                }
            } else if self.desk_tab == super::DeskTab::Recent {
                ui.label(widgets::muted(ui, "Computers you have connected to on this computer."));
            } else if self.cfg.settings.discover_lan {
                ui.label(widgets::muted(
                    ui,
                    "PJDesk computers on this network. Connecting asks for that computer's password and goes directly over the LAN. No account is required.",
                ));
            } else {
                ui.label(widgets::muted(ui, "LAN discovery is turned off in Settings."));
            }
            ui.add_space(6.0);
            let search = widgets::text_field(ui, &mut self.saved_query, "Search nickname, name, ID, or OS", false);
            submitted = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let can_keys = self.cfg.can_manage_support_keys();
                if can_keys && ui.button("Support keys").clicked() {
                    self.show_support_keys = true;
                }
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
                if self.desk_tab == super::DeskTab::Saved {
                    ui.menu_button("Menu", |ui| {
                        if ui.button("Remove all saved computers from this computer").clicked() {
                            self.confirm_clear_saved = true;
                            ui.close();
                        }
                        if ui.button("Sign out and remove everything from this computer").clicked() {
                            self.confirm_sign_out_clear = true;
                            ui.close();
                        }
                    });
                }
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
        let submitted_cmd = if submitted { rows.iter().find(|r| !r.this_computer).map(connect_cmd) } else { None };
        if let Some(cmd) = submitted_cmd {
            self.apply_saved(cmd);
        }

        let view = parse_saved_view(&self.cfg.settings.saved_view);
        theme::card(ui, |ui| {
            if rows.is_empty() {
                let msg = if !query.trim().is_empty() {
                    "No computers match."
                } else if self.desk_tab == super::DeskTab::Lan {
                    if self.cfg.settings.discover_lan {
                        "No PJDesk computers have answered on this network yet."
                    } else {
                        "LAN discovery is turned off in Settings."
                    }
                } else {
                    "No saved computers match."
                };
                ui.label(widgets::muted(ui, msg));
                return;
            }
            let salt = match view {
                SavedView::List => "saved-rows",
                SavedView::Small => "saved-small",
                SavedView::Big => "saved-big",
            };
            egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
                let mut action: Option<RowCmd> = None;
                let available = ui.available_width();
                let width = if available.is_finite() { (available - ROW_GUTTER).max(0.0) } else { 640.0 };
                let layout = saved_layout(width);
                if view == SavedView::List {
                    saved_header(ui, width, &layout);
                }
                let (groups, rest) = client_sections(rows);
                if !groups.is_empty() {
                    ui.label(RichText::new("Clients").strong());
                    ui.add_space(4.0);
                    for (label, group) in &groups {
                        ui.label(widgets::muted(ui, label));
                        self.paint_saved_rows(ui, group, view, width, &layout, &mut action);
                        ui.add_space(8.0);
                    }
                }
                if !rest.is_empty() {
                    self.paint_saved_rows(ui, &rest, view, width, &layout, &mut action);
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

    fn paint_saved_rows(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[SavedRow],
        view: SavedView,
        width: f32,
        layout: &SavedLayout,
        action: &mut Option<RowCmd>,
    ) {
        match view {
            SavedView::List => {
                for row in rows {
                    self.saved_row(ui, row, width, layout, action);
                }
            }
            SavedView::Small => self.tile_grid(ui, rows, false, action),
            SavedView::Big => self.tile_grid(ui, rows, true, action),
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
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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
            let clients = entry.is_some_and(|e| crate::support_keys::is_clients(&e.tags));
            let nickname = crate::config::nickname_text(&alias, &id, &hostname);
            let on_lan = self.lan.iter().any(|(_, lan_id, _)| normalize_id(lan_id) == id);
            let this_computer = self.is_this_computer(&id);
            let last_seen = note.map(|n| n.last_seen).unwrap_or(0);
            let reported = note.is_some_and(|n| n.reported_online);
            // This computer is online when it is registered, or when the
            // current primary says so. Other rows use that primary's flag.
            let online = if this_computer {
                pjdesk_proto::failover::own_row_online(self.registrar_acked(), reported)
            } else {
                presence_online(reported, on_lan)
            };
            let reported_version = note.map(|n| n.app_version.clone()).unwrap_or_default();
            let app_version = row_version(this_computer, &reported_version);
            if !this_computer && pjdesk_proto::presence_stale(last_seen, online, now) {
                continue;
            }
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
                last_seen,
                this_computer,
                sort_order: note.map(|n| n.sort_order).unwrap_or(0),
                display_name: note.map(|n| n.display_name.clone()).unwrap_or_default(),
                in_book: entry.is_some(),
                clients,
                direct: None,
                app_version,
            });
        }
        rows
    }

    fn lan_rows(&self) -> Vec<SavedRow> {
        let mut rows = Vec::new();
        for (name, id, addr) in &self.lan {
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
                clients: entry.is_some_and(|e| crate::support_keys::is_clients(&e.tags)),
                direct: Some(addr.clone()),
                app_version: row_version(
                    self.is_this_computer(&id),
                    &note.map(|n| n.app_version.clone()).unwrap_or_default(),
                ),
            });
        }
        rows
    }

    fn set_saved_view(&mut self, view: SavedView) {
        let key = saved_view_key(view);
        if self.cfg.settings.saved_view == key {
            return;
        }
        self.cfg.settings.saved_view = key.to_string();
        self.save_cfg();
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
            *action = primary_cmd(row);
        }
        let slots = column_slots(rect, layout);
        in_rect(ui, slots.icon, (&id, "icon"), |ui| {
            paint_list_avatar(ui, ui.max_rect(), row);
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
        if let Some(r) = slots.version {
            let text = pjdesk_proto::update::shown_version(&row.app_version);
            let color = version_color(ui, &row.app_version);
            in_rect(ui, r, (&id, "ver"), |ui| {
                ui.add(egui::Label::new(RichText::new(text).color(color).size(12.5)).truncate());
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
                            *action = Some(if android { RowCmd::View(id.to_string()) } else { connect_cmd(row) });
                        }
                    }
                } else if row.this_computer {
                    ui.add_enabled(false, egui::Button::new("Connect"));
                } else if android {
                    if ui.button("View phone").clicked() {
                        *action = Some(RowCmd::View(id.to_string()));
                    }
                } else if ui.button("Connect").clicked() {
                    *action = Some(connect_cmd(row));
                }
            });
        });
        in_rect(ui, menu, (id, "menu"), |ui| {
            ui.set_width(menu.width());
            ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Menu", |ui| {
                    self.menu_items(ui, row, id, action);
                });
            });
        });
    }

    fn menu_items(&mut self, ui: &mut egui::Ui, row: &SavedRow, id: &str, action: &mut Option<RowCmd>) {
                    let can_keys = self.cfg.can_manage_support_keys();
                    if can_keys && ui.button("Support keys").clicked() {
                        self.show_support_keys = true;
                        ui.close();
                    }
                    if can_keys && row.clients && ui.button("Remove from this support key").clicked() {
                        *action = Some(RowCmd::RemoveSupport(id.to_string()));
                        ui.close();
                    }
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
    }

    fn tile_grid(&mut self, ui: &mut egui::Ui, rows: &[SavedRow], big: bool, action: &mut Option<RowCmd>) {
        let gap = 10.0;
        let min_w = if big { 300.0 } else { 230.0 };
        let avail = ui.available_width();
        let avail = if avail.is_finite() { avail.max(min_w) } else { min_w * 2.0 + gap };
        let cols = ((avail + gap) / (min_w + gap)).floor().max(1.0) as usize;
        let card_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
        let card_h = if big { 228.0 } else { 148.0 };
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(avail);
            ui.spacing_mut().item_spacing = Vec2::splat(gap);
            for row in rows {
                self.tile_card(ui, row, card_w, card_h, big, action);
            }
        });
    }

    fn tile_card(&mut self, ui: &mut egui::Ui, row: &SavedRow, w: f32, h: f32, big: bool, action: &mut Option<RowCmd>) {
        let p = theme::paint(ui);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let id = row.id.clone();
        if resp.double_clicked() && self.renaming.as_deref() != Some(id.as_str()) {
            if let Some(cmd) = primary_cmd(row) {
                *action = Some(cmd);
            }
        }
        if resp.hovered() {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(12), p.text.linear_multiply(0.04));
        }
        let color = tile_color(&tile_key(&row.id, &row.hostname));
        let header_h = if big { h * 0.70 } else { h * 0.60 };
        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_h));
        let footer = Rect::from_min_max(Pos2::new(rect.left(), header.bottom()), rect.right_bottom());
        ui.painter().rect_filled(
            header,
            egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 },
            color,
        );
        let foot = if resp.hovered() { p.elev } else { p.card };
        ui.painter().rect_filled(footer, egui::CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 }, foot);
        ui.painter().rect_stroke(rect, egui::CornerRadius::same(12), egui::Stroke::new(1.0, p.line), egui::StrokeKind::Inside);

        let caption = tile_caption(&row.console_user, &row.hostname, &row.display_name, &row.nickname, &row.id);
        let icon_side = if big { 72.0 } else { 36.0 };
        let icon_y = if big { header.center().y - 16.0 } else { header.top() + header_h * 0.38 };
        let icon_rect = Rect::from_center_size(Pos2::new(header.center().x, icon_y), Vec2::splat(icon_side));
        super::icons::paint(ui.painter(), icon_rect, os_icon(&row.os_family), egui::Color32::WHITE);
        let text_rect = Rect::from_min_max(
            Pos2::new(header.left() + 10.0, icon_rect.bottom() + 2.0),
            Pos2::new(header.right() - 10.0, header.bottom() - 6.0),
        );
        in_rect(ui, text_rect, (&id, "cap"), |ui| {
            ui.set_min_width(text_rect.width().max(0.0));
            ui.set_max_width(text_rect.width().max(0.0));
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(caption).color(egui::Color32::WHITE).size(if big { 14.0 } else { 12.0 }))
                        .truncate(),
                );
                if big {
                    let text = pjdesk_proto::update::shown_version(&row.app_version);
                    let color = version_on_tile(ui, &row.app_version);
                    ui.add(egui::Label::new(RichText::new(text).color(color).size(12.0)).truncate());
                }
            });
        });

        let dot = status_color(ui, row);
        ui.painter().circle_filled(Pos2::new(footer.left() + 16.0, footer.center().y), 4.5, dot);
        let menu_w = 32.0;
        let connect_w = if big { 96.0 } else { 36.0 };
        let right = footer.right() - 4.0;
        let menu_rect = Rect::from_min_max(Pos2::new(right - menu_w, footer.top()), Pos2::new(right, footer.bottom()));
        let connect_rect = Rect::from_min_max(Pos2::new(menu_rect.left() - connect_w - 2.0, footer.top()), menu_rect.left_bottom());
        let name_rect = Rect::from_min_max(Pos2::new(footer.left() + 28.0, footer.top()), Pos2::new(connect_rect.left() - 4.0, footer.bottom()));
        let renaming = self.renaming.as_deref() == Some(id.as_str());
        if renaming {
            if name_editor(ui, name_rect, &id, &mut self.rename_text) {
                *action = Some(RowCmd::CommitRename(id.clone()));
            }
        } else {
            let primary = row_primary_name(row);
            let placeholder = primary == "No name yet";
            in_rect(ui, name_rect, (&id, "tile-name"), |ui| {
                ui.set_max_width(name_rect.width().max(0.0));
                let color = if placeholder { p.muted } else { p.text };
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(primary).color(color).size(13.0)).truncate());
                    let text = pjdesk_proto::update::shown_version(&row.app_version);
                    let ver = version_color(ui, &row.app_version);
                    ui.add(egui::Label::new(RichText::new(text).color(ver).size(11.0)).truncate());
                });
            });
        }
        let android = row.os_family.eq_ignore_ascii_case("android");
        in_rect(ui, connect_rect, (&id, "tile-connect"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if row.this_computer {
                    if big {
                        ui.add_enabled(false, egui::Button::new("Connect"));
                    } else {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        super::icons::paint(ui.painter(), r.shrink(6.0), Icon::Go, p.muted);
                        resp.on_hover_text("This computer");
                    }
                } else if big {
                    let label = if android { "View" } else { "Connect" };
                    if ui.button(label).clicked() {
                        *action = Some(if android { RowCmd::View(id.clone()) } else { connect_cmd(row) });
                    }
                } else {
                    let tip = if android { "View phone" } else { "Connect" };
                    let resp = widgets::icon_button(ui, Icon::Go, tip);
                    if resp.clicked() {
                        *action = Some(if android { RowCmd::View(id.clone()) } else { connect_cmd(row) });
                    }
                }
            });
        });
        in_rect(ui, menu_rect, (&id, "tile-menu"), |ui| {
            let button = egui::Button::new(" ").min_size(Vec2::splat(28.0)).frame(false);
            let (resp, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                self.menu_items(ui, row, &id, action);
            });
            let color = if resp.hovered() { p.text } else { p.muted };
            super::icons::paint(ui.painter(), resp.rect.shrink(6.0), Icon::Dots, color);
            resp.on_hover_text("Menu");
        });
    }

    fn apply_saved(&mut self, cmd: RowCmd) {
        match cmd {
            RowCmd::Connect(id) => {
                let client = self.cfg.address_book.iter().any(|e| e.id == id && crate::support_keys::is_clients(&e.tags));
                if client && self.cfg.saved_password(&id).is_none() {
                    self.ask_person_next = true;
                    self.spawn_viewer(&id, "", false, false, false);
                } else {
                    self.connect_to(&id, None, false);
                }
            }
            RowCmd::Direct(id, addr) => self.connect_discovered(&id, &addr),
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
            RowCmd::RemoveSupport(id) => self.remove_support_computer(&id),
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

    fn remove_support_computer(&mut self, id: &str) {
        if self.support_keys.busy {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            self.status = "Sign in to remove a computer from a support key.".into();
            return;
        }
        let id = id.to_string();
        let sent = id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.support_keys.busy = true;
        self.support_keys.rx = Some(rx);
        self.support_keys.note = "Working…".into();
        std::thread::spawn(move || {
            let job = match crate::support_keys::remove_computer_by_id(&refresh, &device, &sent) {
                Ok(()) => match crate::support_keys::list_keys(&refresh, &device) {
                    Ok(keys) => crate::support_keys::Job::Listed(keys),
                    Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                },
                Err(e) => crate::support_keys::Job::Failed(e.to_string()),
            };
            let _ = tx.send(job);
        });
        self.cfg.address_book.retain(|entry| entry.id != id);
        if !self.cfg.saved_hidden.iter().any(|hidden| hidden == &id) {
            self.cfg.saved_hidden.push(id);
        }
        let _ = self.cfg.save();
        self.request_saved_refresh();
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

    pub(super) fn saved_clear_dialogs(&mut self, ctx: &egui::Context) {
        if self.confirm_clear_saved {
            self.clear_saved_dialog(ctx, false);
        }
        if self.confirm_sign_out_clear {
            self.clear_saved_dialog(ctx, true);
        }
    }

    fn clear_saved_dialog(&mut self, ctx: &egui::Context, sign_out: bool) {
        let count = self.cfg.local_saved_count();
        let noun = if count == 1 { "computer" } else { "computers" };
        let title = if sign_out { "Sign out and remove everything" } else { "Remove all saved computers" };
        let mut close = false;
        let mut go = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(format!("This computer has {count} saved {noun}."));
                ui.label(widgets::muted(
                    ui,
                    if sign_out {
                        "This signs out on this computer and clears the saved list, cached passwords, recent computers, and saved addresses here. Nothing is deleted from your pjdesk.com account or from your other computers."
                    } else {
                        "This clears the saved list, cached passwords, recent computers, and saved addresses on this computer only. Nothing is deleted from your pjdesk.com account or from your other computers."
                    },
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let label = if sign_out { "Sign out and remove" } else { "Remove from this computer" };
                    if widgets::danger_button(ui, label, Vec2::new(220.0, 32.0)) {
                        go = true;
                    }
                });
            });
        if close {
            self.confirm_clear_saved = false;
            self.confirm_sign_out_clear = false;
        }
        if go {
            self.confirm_clear_saved = false;
            self.confirm_sign_out_clear = false;
            if sign_out {
                self.sign_out_and_clear_local();
            } else {
                self.clear_saved_on_this_computer();
            }
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
        let direct_addr = self.prompt_direct.clone();
        let named_peer = direct_addr.is_some() && pjdesk_proto::is_valid_id(&self.prompt_id);
        let name = if direct_addr.is_some() && !named_peer {
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
                let id_line = if let Some(addr) = &direct_addr {
                    if named_peer {
                        format!("{} · {addr}", format_id(&self.prompt_id))
                    } else {
                        self.prompt_id.clone()
                    }
                } else {
                    format_id(&self.prompt_id)
                };
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
                if direct_addr.is_some() {
                    ui.label(widgets::muted(
                        ui,
                        "That computer's permanent password, or a one-time support code. No PJDesk account is required. The connection stays on this network, and discovery never includes the password.",
                    ));
                } else {
                    ui.label(widgets::muted(
                        ui,
                        "The password on this computer, not your PJDesk account. Saved on this computer only, and not uploaded.",
                    ));
                }
                ui.add_space(8.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if !self.prompt_checking && !named_peer && ui.button("Ask to connect instead").clicked() {
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
                if let Some(addr) = self.prompt_direct.clone() {
                    self.spawn_direct_with_password(&addr, &pw);
                } else {
                    self.spawn_viewer(&id, &pw, files, remember, false);
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RowCmd {
    Connect(String),
    /// Computer id and the direct LAN address. Asks for that computer's password.
    Direct(String, String),
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
    RemoveSupport(String),
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
const VER_PREF: f32 = 72.0;
const VER_MIN: f32 = 52.0;
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

fn connect_cmd(row: &SavedRow) -> RowCmd {
    match row.direct.as_deref().map(str::trim).filter(|addr| !addr.is_empty()) {
        Some(addr) => RowCmd::Direct(row.id.clone(), addr.to_string()),
        None => RowCmd::Connect(row.id.clone()),
    }
}

fn primary_cmd(row: &SavedRow) -> Option<RowCmd> {
    match primary_action(row.this_computer, &row.os_family) {
        Some(PrimaryAction::Connect) => Some(connect_cmd(row)),
        Some(PrimaryAction::View) => Some(RowCmd::View(row.id.clone())),
        None => None,
    }
}

fn row_primary_name(row: &SavedRow) -> String {
    if !row.nickname.is_empty() {
        row.nickname.clone()
    } else if !row.display_name.is_empty() {
        row.display_name.clone()
    } else if !row.hostname.is_empty() {
        row.hostname.clone()
    } else {
        "No name yet".into()
    }
}

fn paint_list_avatar(ui: &egui::Ui, rect: Rect, row: &SavedRow) {
    let color = tile_color(&tile_key(&row.id, &row.hostname));
    let side = 26.0_f32.min((rect.height() - 8.0).max(8.0)).min(rect.width().max(8.0));
    let avatar = Rect::from_center_size(rect.center(), Vec2::splat(side));
    ui.painter().rect_filled(avatar, egui::CornerRadius::same(6), color);
    super::icons::paint(ui.painter(), avatar.shrink(5.0), os_icon(&row.os_family), egui::Color32::WHITE);
    let at = Pos2::new(avatar.right() - 1.0, avatar.bottom() - 1.0);
    ui.painter().circle_filled(at, 4.2, theme::paint(ui).card);
    ui.painter().circle_filled(at, 3.0, status_color(ui, row));
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
    show_version: bool,
    show_id: bool,
    status_in_name: bool,
    compact_connect: bool,
    name: f32,
    user: f32,
    last: f32,
    system_text: f32,
    version: f32,
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
        if self.show_version {
            add(self.version);
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
/// User, Last connected, the system text, the version, then the ID column
/// drop in that order. The ID then sits under the name. The status dot folds
/// into the name after that. Connect becomes an icon below [`COMPACT_BELOW`].
fn saved_layout(available: f32) -> SavedLayout {
    let available = if available.is_finite() { available.max(0.0) } else { 800.0 };
    let compact = available < COMPACT_BELOW;
    let stages = [
        (true, true, true, true, true, true),
        (false, true, true, true, true, true),
        (false, false, true, true, true, true),
        (false, false, false, true, true, true),
        (false, false, false, false, true, true),
        (false, false, false, false, false, true),
        (false, false, false, false, false, false),
    ];
    for (user, last, sys, version, id, status_beside) in stages {
        if let Some(layout) = fit(available, user, last, sys, version, id, status_beside, compact) {
            return layout;
        }
    }
    fit(available, false, false, false, false, false, false, true).unwrap_or(SavedLayout {
        show_user: false,
        show_last: false,
        show_system_text: false,
        show_version: false,
        show_id: false,
        status_in_name: true,
        compact_connect: true,
        name: 0.0,
        user: 0.0,
        last: 0.0,
        system_text: 0.0,
        version: 0.0,
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
    version: bool,
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
    if version {
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
    if version {
        cols.push(Flex { pref: VER_PREF, min: VER_MIN });
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
    let version_w = version.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let id_w = id.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let status_w = status_beside.then(|| next.next().unwrap_or(0.0)).unwrap_or(0.0);
    let name = next.next().unwrap_or(NAME_MIN);
    Some(SavedLayout {
        show_user: user,
        show_last: last,
        show_system_text: sys,
        show_version: version,
        show_id: id,
        status_in_name: !status_beside,
        compact_connect: compact,
        name,
        user: user_w,
        last: last_w,
        system_text: sys_w,
        version: version_w,
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
    version: Option<Rect>,
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
    let version = if layout.show_version { Some(take(layout.version)) } else { None };
    let system_text = if layout.show_system_text { Some(take(layout.system_text)) } else { None };
    let os = take(W_OS);
    let last = if layout.show_last { Some(take(layout.last)) } else { None };
    let user = if layout.show_user { Some(take(layout.user)) } else { None };
    let copy = if layout.show_id { Some(take(W_COPY)) } else { None };
    let id = if layout.show_id { Some(take(layout.id)) } else { None };
    let icon = Rect::from_min_size(content.left_top(), Vec2::new(W_ICON, content.height()));
    let name_left = icon.right() + GAP;
    let name = Rect::from_min_max(Pos2::new(name_left, content.top()), Pos2::new(edge.max(name_left), content.bottom()));
    Slots { icon, name, id, copy, user, last, os, system_text, version, lock, status, connect, menu }
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
    if row.online {
        "Online".into()
    } else if row.last_seen == 0 {
        "Offline".into()
    } else {
        format!("Offline · {}", ago(row.last_seen))
    }
}

fn version_color(ui: &egui::Ui, version: &str) -> egui::Color32 {
    let p = theme::paint(ui);
    match cached_latest() {
        Some(latest) if pjdesk_proto::update::version_is_behind(version, &latest) => p.warn,
        _ => p.muted,
    }
}

/// Amber on a colored tile when the version is behind. White when it is current or unknown.
fn version_on_tile(ui: &egui::Ui, version: &str) -> egui::Color32 {
    match cached_latest() {
        Some(latest) if pjdesk_proto::update::version_is_behind(version, &latest) => theme::paint(ui).warn,
        _ => egui::Color32::WHITE,
    }
}

fn cached_latest() -> Option<String> {
    // The primary fetches the release and tells connected clients. This window
    // does not call pjdesk.com to color a version.
    None
}

fn status_color(ui: &egui::Ui, row: &SavedRow) -> egui::Color32 {
    let p = theme::paint(ui);
    match status_tone(row.online, row.this_computer) {
        StatusTone::Ok => p.ok,
        StatusTone::Warn => p.warn,
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
    if let Some(r) = slots.version {
        label(ui, r, "ver", "Version");
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
                clients: false,
                direct: None,
                app_version: String::new(),
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
                clients: false,
                direct: None,
                app_version: String::new(),
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
    fn status_uses_live_presence_not_last_connected() {
        let mut row = SavedRow {
            id: "902488771".into(),
            nickname: "Cabin Desk".into(),
            hostname: "CABINDESK".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            last_connected: u64::MAX,
            favorite: false,
            has_password: false,
            online: false,
            console_user: String::new(),
            last_seen: 0,
            this_computer: false,
            sort_order: 0,
            display_name: String::new(),
            in_book: true,
            clients: false,
            direct: None,
            app_version: String::new(),
        };
        assert_eq!(status_text(&row), "Offline");
        assert!(!presence_online(false, false));
        row.online = presence_online(true, false);
        row.last_connected = 0;
        assert_eq!(status_text(&row), "Online");
        assert!(presence_online(false, true));
    }

    #[test]
    fn this_computer_shows_the_running_version() {
        let local = env!("CARGO_PKG_VERSION");
        assert_eq!(row_version(true, "0.6.43"), local);
        assert_eq!(row_version(true, ""), local);
        assert_eq!(row_version(false, "0.6.43"), "0.6.43");
        assert_eq!(row_version(false, ""), "");
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
        assert!(prev.show_user && prev.show_last && prev.show_system_text && prev.show_version && prev.show_id);
        assert!(!prev.status_in_name && !prev.compact_connect);
        let mut dropped_user = None;
        let mut dropped_last = None;
        let mut dropped_sys = None;
        let mut dropped_version = None;
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
                assert!(prev.show_version || !layout.show_version, "version came back at {w}");
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
            if dropped_version.is_none() && !layout.show_version {
                dropped_version = Some(w);
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
        let version = dropped_version.expect("version hides");
        let id = dropped_id.expect("id column hides");
        let status = merged_status.expect("status merges into the name");
        assert!(
            user > last && last > sys && sys > version && version > id && id > status,
            "hide order user {user} last {last} sys {sys} version {version} id {id} status {status}"
        );
        assert!(!saved_layout(COMPACT_BELOW).compact_connect);
        assert!(saved_layout(COMPACT_BELOW - 1.0).compact_connect);
    }

    #[test]
    fn tile_color_stays_with_the_id() {
        assert_eq!(tile_index(&tile_key("692996394", "CABINDESK")), tile_index("692996394"));
        assert_eq!(tile_index("692 996 394"), tile_index("692996394"));
        let mut seen = std::collections::BTreeSet::new();
        for id in ["692996394", "622775091", "902488771", "386626882", "482636105", "111111111", "222222222"] {
            seen.insert(tile_index(id));
        }
        assert!(seen.len() > 1, "palette collapsed: {seen:?}");
        assert_eq!(tile_key("", "Cabindesk"), "cabindesk");
        assert_eq!(tile_index(&tile_key("", "Cabindesk")), tile_index("cabindesk"));
        assert_eq!(tile_color("692996394"), tile_color("692996394"));
    }

    #[test]
    fn offline_dot_is_the_warn_tone() {
        assert_eq!(status_tone(true, false), StatusTone::Ok);
        assert_eq!(status_tone(false, true), StatusTone::Warn, "this computer is not online without a server ack");
        assert_eq!(status_tone(true, true), StatusTone::Ok);
        assert_eq!(status_tone(false, false), StatusTone::Warn);
        let mut here = sample_row("902488771", None);
        here.this_computer = true;
        here.online = false;
        here.last_seen = 0;
        assert_eq!(status_text(&here), "Offline");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        here.last_seen = now.saturating_sub(120);
        assert_eq!(status_text(&here), "Offline · 2 min ago");
        here.online = true;
        assert_eq!(status_text(&here), "Online");
        assert!(presence_refresh_line(&[]).contains("no computers"));
    }

    #[test]
    fn saved_view_defaults_to_the_list() {
        assert_eq!(parse_saved_view(""), SavedView::List);
        assert_eq!(parse_saved_view("list"), SavedView::List);
        assert_eq!(parse_saved_view("nope"), SavedView::List);
        assert_eq!(parse_saved_view("small"), SavedView::Small);
        assert_eq!(parse_saved_view("big"), SavedView::Big);
        assert_eq!(saved_view_key(SavedView::List), "list");
        assert_eq!(saved_view_key(SavedView::Small), "small");
        assert_eq!(saved_view_key(SavedView::Big), "big");
        let settings: crate::config::Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.discover_lan);
        assert!(settings.allow_direct);
        assert_eq!(parse_saved_view(&settings.saved_view), SavedView::List);
        let mut off = settings;
        off.discover_lan = false;
        let args = crate::HostArgs::from_settings(&off);
        assert!(!args.discover_lan);
        assert!(args.allow_direct);
    }

    #[test]
    fn discovered_connect_uses_the_lan_address() {
        let saved = sample_row("692996394", None);
        assert!(matches!(connect_cmd(&saved), RowCmd::Connect(id) if id == "692996394"));
        let found = sample_row("692996394", Some("192.168.1.20:21201"));
        match connect_cmd(&found) {
            RowCmd::Direct(id, addr) => {
                assert_eq!(id, "692996394");
                assert_eq!(addr, "192.168.1.20:21201");
            }
            other => panic!("expected a direct connect, got {other:?}"),
        }
        assert!(matches!(primary_cmd(&found), Some(RowCmd::Direct(_, _))));
        let mut here = found.clone();
        here.this_computer = true;
        assert_eq!(primary_cmd(&here), None);
        assert_eq!(tile_caption("Ben", "CABINDESK", "", "", "692996394"), "Ben@CABINDESK");
        assert_eq!(tile_caption("", "CABINDESK", "", "", "692996394"), "CABINDESK");
    }

    fn sample_row(id: &str, direct: Option<&str>) -> SavedRow {
        SavedRow {
            id: id.into(),
            nickname: String::new(),
            hostname: "CABINDESK".into(),
            os_family: "windows".into(),
            os_version: String::new(),
            last_connected: 0,
            favorite: false,
            has_password: false,
            online: true,
            console_user: "Ben".into(),
            last_seen: 0,
            this_computer: false,
            sort_order: 0,
            display_name: String::new(),
            in_book: false,
            clients: false,
            direct: direct.map(str::to_string),
            app_version: String::new(),
        }
    }
}
