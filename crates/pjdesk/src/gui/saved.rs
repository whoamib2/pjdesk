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
}

pub struct SavedRow {
    pub id: String,
    pub name: String,
    pub os_family: String,
    pub os_version: String,
    pub last_connected: u64,
    pub favorite: bool,
    pub has_password: bool,
    pub online: bool,
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

/// True when `query` matches the name, the ID (spaces ignored), or the OS.
pub fn query_matches(query: &str, name: &str, id: &str, os_family: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    let q_id: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let id_compact: String = id.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    name.to_ascii_lowercase().contains(&q)
        || (!q_id.is_empty() && id_compact.contains(&q_id))
        || os_label(os_family).to_ascii_lowercase().contains(&q)
        || os_family.to_ascii_lowercase().contains(&q)
}

pub fn sort_rows(rows: &mut [SavedRow], sort: SavedSort) {
    match sort {
        SavedSort::Name => rows.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()).then(a.id.cmp(&b.id))),
        SavedSort::Recent => rows.sort_by(|a, b| b.last_connected.cmp(&a.last_connected).then(a.name.cmp(&b.name))),
        SavedSort::Os => rows.sort_by(|a, b| os_label(&a.os_family).cmp(os_label(&b.os_family)).then(a.name.cmp(&b.name))),
    }
}

impl App {
    pub(super) fn saved_page(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let mut submitted = false;
        theme::card(ui, |ui| {
            ui.label(RichText::new("Saved computers").size(22.0).strong().color(p.text));
            ui.label(widgets::muted(ui, "Search by name, ID, or operating system. Enter connects to the first match."));
            ui.add_space(8.0);
            let search = widgets::text_field(ui, &mut self.saved_query, "Search name, ID, or OS", false);
            submitted = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(widgets::muted(ui, "Sort"));
                egui::ComboBox::from_id_salt("saved-sort")
                    .selected_text(match self.saved_sort {
                        SavedSort::Name => "Name",
                        SavedSort::Recent => "Last connected",
                        SavedSort::Os => "Operating system",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Name, "Name");
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Recent, "Last connected");
                        ui.selectable_value(&mut self.saved_sort, SavedSort::Os, "Operating system");
                    });
                widgets::checkbox(ui, &mut self.saved_favorites, "Favorites");
            });
        });
        ui.add_space(12.0);

        let mut rows = self.saved_rows();
        if self.saved_favorites {
            rows.retain(|r| r.favorite);
        }
        let query = self.saved_query.clone();
        rows.retain(|r| query_matches(&query, &r.name, &r.id, &r.os_family));
        sort_rows(&mut rows, self.saved_sort);
        if submitted {
            if let Some(first) = rows.first() {
                let id = first.id.clone();
                self.connect_to(&id, None, false);
            }
        }

        theme::card(ui, |ui| {
            if rows.is_empty() {
                ui.label(widgets::muted(ui, "No saved computers match."));
                return;
            }
            header(ui);
            ui.separator();
            egui::ScrollArea::vertical().id_salt("saved-rows").auto_shrink([false, false]).show(ui, |ui| {
                let mut action: Option<RowCmd> = None;
                for row in &rows {
                    if self.row_line(ui, row, &mut action) {
                        ui.separator();
                    }
                }
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
            if !pjdesk_proto::is_valid_id(&id) {
                continue;
            }
            let entry = self.cfg.address_book.iter().find(|e| e.id == id);
            let note = self.cfg.settings.peers.get(&id);
            let alias = entry.and_then(|e| {
                let a = e.alias.trim();
                if a.is_empty() { None } else { Some(a.to_string()) }
            });
            let name = alias.unwrap_or_else(|| format_id(&id));
            let online = self.lan.iter().any(|(_, lan_id, _)| normalize_id(lan_id) == id);
            rows.push(SavedRow {
                id: id.clone(),
                name,
                os_family: note.map(|n| n.os_family.clone()).unwrap_or_default(),
                os_version: note.map(|n| n.os_version.clone()).unwrap_or_default(),
                last_connected: note.map(|n| n.last_connected).unwrap_or(0),
                favorite: note.map(|n| n.favorite).unwrap_or(false),
                has_password: self.cfg.saved_password(&id).is_some(),
                online,
            });
        }
        rows
    }

    fn row_line(&mut self, ui: &mut egui::Ui, row: &SavedRow, action: &mut Option<RowCmd>) -> bool {
        let p = theme::paint(ui);
        let id = row.id.clone();
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
            super::icons::paint(ui.painter(), rect, os_icon(&row.os_family), p.text);
            ui.add_space(8.0);
            if self.renaming.as_deref() == Some(id.as_str()) {
                let edit = ui.add(egui::TextEdit::singleline(&mut self.rename_text).desired_width(COL_NAME));
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    *action = Some(RowCmd::CommitRename(id.clone()));
                }
            } else {
                left_cell(ui, COL_NAME, RichText::new(&row.name).color(p.text));
            }
            left_cell(ui, COL_ID, RichText::new(format_id(&id)).monospace().color(p.muted));
            if widgets::icon_button(ui, Icon::Copy, "Copy ID").clicked() {
                ui.ctx().copy_text(format_id(&id));
                self.toast.show("Copied");
            }
            let when = if row.last_connected == 0 { "Never".to_string() } else { ago(row.last_connected) };
            ui.add_sized([110.0, 20.0], egui::Label::new(RichText::new(when).color(p.muted).size(12.0)));
            let os = if row.os_version.is_empty() {
                os_label(&row.os_family).to_string()
            } else {
                format!("{} {}", os_label(&row.os_family), row.os_version)
            };
            ui.add_sized([140.0, 20.0], egui::Label::new(RichText::new(os).color(p.muted).size(12.0)));
            if row.has_password {
                theme::icon_in(ui, Icon::Lock, 14.0, p.muted);
            } else {
                ui.add_space(18.0);
            }
            if row.online {
                widgets::status_dot(ui, p.ok, "On this network");
            } else {
                ui.add_space(14.0);
            }
            if ui.button("Connect").clicked() {
                *action = Some(RowCmd::Connect(id.clone()));
            }
            ui.menu_button("Menu", |ui| {
                if ui.button("Rename").clicked() {
                    *action = Some(RowCmd::Rename(id.clone()));
                    ui.close();
                }
                if ui.button("Transfer file").clicked() {
                    *action = Some(RowCmd::Files(id.clone()));
                    ui.close();
                }
                let fav = if row.favorite { "Unfavorite" } else { "Favorite" };
                if ui.button(fav).clicked() {
                    *action = Some(RowCmd::Favorite(id.clone()));
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
        true
    }

    fn apply_saved(&mut self, cmd: RowCmd) {
        match cmd {
            RowCmd::Connect(id) => self.connect_to(&id, None, false),
            RowCmd::Files(id) => self.connect_to(&id, None, true),
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
            RowCmd::Rename(id) => {
                self.rename_text = self.row_title(&id);
                self.renaming = Some(id);
            }
            RowCmd::CommitRename(id) => {
                let alias = self.rename_text.trim().to_string();
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
                self.toast.show("Name saved");
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
            self.cfg.address_book.retain(|e| e.id != id);
            self.cfg.remember_password(&id, None);
            self.cfg.settings.peers.remove(&id);
            self.confirm_remove = None;
            self.sync_address_book();
            self.toast.show("Removed");
        }
    }

    pub(super) fn password_prompt(&mut self, ctx: &egui::Context) {
        if !self.prompt_open {
            return;
        }
        let mut close = false;
        let mut go = false;
        let title = if self.prompt_rejected { "Password was not accepted" } else { "Password" };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let name = self.row_title(&self.prompt_id.clone());
                ui.label(RichText::new(name).strong());
                ui.label(RichText::new(format_id(&self.prompt_id)).monospace().color(theme::paint(ui).muted));
                if self.prompt_rejected {
                    ui.add_space(4.0);
                    ui.label("That saved password was rejected. Enter the current one to update it.");
                }
                ui.add_space(8.0);
                widgets::field_label(ui, "Password");
                let resp = widgets::password_field(ui, &mut self.prompt_password, "Password", &mut self.prompt_show);
                ui.add_space(4.0);
                widgets::checkbox(ui, &mut self.prompt_remember, "Remember password");
                ui.add_space(8.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if widgets::accent_button(ui, "Connect", Vec2::new(120.0, 36.0)) || enter {
                        go = true;
                    }
                });
            });
        if close {
            self.prompt_open = false;
        }
        if go {
            let (id, pw, files, remember) = (
                self.prompt_id.clone(),
                self.prompt_password.clone(),
                self.prompt_files,
                self.prompt_remember,
            );
            if pw.trim().is_empty() {
                self.status = "Enter the password.".into();
            } else {
                self.prompt_open = false;
                self.remember = remember;
                self.spawn_viewer(&id, &pw, files, remember, false);
            }
        }
    }
}

enum RowCmd {
    Connect(String),
    Files(String),
    Favorite(String),
    ForgetPassword(String),
    Remove(String),
    Rename(String),
    CommitRename(String),
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

const COL_NAME: f32 = 210.0;
const COL_ID: f32 = 156.0;

fn left_cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(Vec2::new(width, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add(egui::Label::new(text).truncate());
    });
}

fn header(ui: &mut egui::Ui) {
    let p = theme::paint(ui);
    ui.horizontal(|ui| {
        ui.add_space(32.0);
        left_cell(ui, COL_NAME, RichText::new("Name").size(12.0).color(p.muted).strong());
        left_cell(ui, COL_ID, RichText::new("ID").size(12.0).color(p.muted).strong());
        ui.add_space(28.0);
        left_cell(ui, 110.0, RichText::new("Last connected").size(12.0).color(p.muted).strong());
        left_cell(ui, 140.0, RichText::new("System").size(12.0).color(p.muted).strong());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_name_id_with_spaces_and_os() {
        assert!(query_matches("office", "Office PC", "123456789", "windows"));
        assert!(query_matches("123 456", "Office PC", "123456789", "windows"));
        assert!(query_matches("linux", "Lab", "555666777", "linux"));
        assert!(query_matches("mac", "Studio", "555666777", "macos"));
        assert!(!query_matches("android", "Office PC", "123456789", "windows"));
        assert!(query_matches("   ", "Office PC", "123456789", ""));
    }

    #[test]
    fn sort_puts_favorites_recent_first_when_asked() {
        let mut rows = vec![
            SavedRow {
                id: "222222222".into(),
                name: "Zed".into(),
                os_family: "linux".into(),
                os_version: String::new(),
                last_connected: 10,
                favorite: false,
                has_password: false,
                online: false,
            },
            SavedRow {
                id: "111111111".into(),
                name: "Alpha".into(),
                os_family: "windows".into(),
                os_version: String::new(),
                last_connected: 50,
                favorite: true,
                has_password: true,
                online: false,
            },
        ];
        sort_rows(&mut rows, SavedSort::Name);
        assert_eq!(rows[0].name, "Alpha");
        sort_rows(&mut rows, SavedSort::Recent);
        assert_eq!(rows[0].id, "111111111");
        sort_rows(&mut rows, SavedSort::Os);
        assert_eq!(rows[0].os_family, "linux");
    }
}
