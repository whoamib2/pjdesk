//! Settings: a centered page with a sidebar, a scrolling body, and a fixed footer.
//!
//! Opening it does no I/O. The auto-update checkbox and the service status come
//! from the background probe; until that answer arrives the controls say so.

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};

use super::{theme, widgets, App};
use crate::config::Config;
use crate::signaling::ServerAddr;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    General,
    Security,
    Privacy,
    Network,
    Display,
    Account,
    About,
}

/// An editable copy of the settings, committed only when Save is pressed.
pub struct SettingsDraft {
    tab: Tab,
    server: String,
    start_with_windows: bool,
    remember_passwords: bool,
    temp_idle_minutes: u32,
    view_only: bool,
    use_permanent: bool,
    permanent_password: String,
    fps: u32,
    bitrate_kbps: u32,
    max_width: u32,
    force_gdi: bool,
    service_password: String,
    service_note: String,
    show_permanent: bool,
    show_service_password: bool,
    auto_update: bool,
    /// True once we have heard from the probe (or the caller passed a cached value).
    auto_update_known: bool,
    /// The user moved the checkbox, so a later probe result must not overwrite it.
    auto_update_touched: bool,
    theme: String,
    approval: String,
    receive_dir: String,
    computer_name: String,
    allow_direct: bool,
    discover_lan: bool,
    tunnel_ports: String,
    access_key: String,
    show_access_key: bool,
}

impl SettingsDraft {
    pub(super) fn show_about(&mut self) {
        self.tab = Tab::About;
    }

    #[allow(dead_code)]
    pub(super) fn show_security(&mut self) {
        self.tab = Tab::Security;
    }

    /// `update_task` is the cached probe result. `None` means "still checking"
    /// — this function does not ask Windows.
    pub fn from_config(cfg: &Config, update_task: Option<bool>) -> Self {
        let s = &cfg.settings;
        Self {
            tab: Tab::General,
            server: cfg.server.clone().unwrap_or_default(),
            start_with_windows: s.start_with_windows,
            remember_passwords: s.remember_passwords,
            temp_idle_minutes: s.temp_idle_minutes.clamp(1, 24 * 60),
            view_only: s.view_only,
            use_permanent: s.permanent_password.is_some(),
            permanent_password: s.permanent_password().unwrap_or_default(),
            fps: s.fps,
            bitrate_kbps: s.bitrate_kbps,
            max_width: s.max_width,
            force_gdi: s.force_gdi,
            service_password: String::new(),
            service_note: String::new(),
            show_permanent: false,
            show_service_password: false,
            auto_update: update_task.unwrap_or(false),
            auto_update_known: update_task.is_some(),
            auto_update_touched: false,
            theme: normalize_theme(&s.theme),
            approval: if s.approval == "unattended" { "unattended".into() } else { "prompt".into() },
            receive_dir: s.receive_dir.clone(),
            computer_name: crate::hostfacts::read_computer_name(),
            allow_direct: s.allow_direct,
            discover_lan: s.discover_lan,
            tunnel_ports: s.tunnel_ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "),
            access_key: cfg.server_access_key(),
            show_access_key: false,
        }
    }

    #[cfg(feature = "ui-shots")]
    pub(crate) fn preview_security(&mut self) {
        self.tab = Tab::Security;
    }

    #[cfg(feature = "ui-shots")]
    pub(crate) fn preview_network(&mut self) {
        self.tab = Tab::Network;
    }

    pub(crate) fn set_server_text(&mut self, server: &str) {
        self.server = server.to_string();
    }
}

fn normalize_theme(theme: &str) -> String {
    match theme {
        "dark" | "light" | "system" => theme.to_string(),
        _ => "system".into(),
    }
}

impl App {
    pub(super) fn open_settings(&mut self) {
        let cached = self.probe.as_ref().and_then(|p| p.latest.update_task);
        self.settings = SettingsDraft::from_config(&self.cfg, cached);
        self.show_settings = true;
        if cached.is_none() {
            if let Some(probe) = &self.probe {
                probe.request_update_task();
            }
        }
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        if self.settings.auto_update_known {
            // already filled
        } else if let Some(on) = self.probe.as_ref().and_then(|p| p.latest.update_task) {
            if !self.settings.auto_update_touched {
                self.settings.auto_update = on;
                self.settings.auto_update_known = true;
            }
        }

        let mut save = false;
        let mut cancel = false;
        let screen = ctx.content_rect();
        let size = Vec2::new((screen.width() - 48.0).clamp(720.0, 920.0), (screen.height() - 48.0).clamp(520.0, 680.0));
        let p = theme::paint_ctx(ctx);

        let layer = egui::Area::new(egui::Id::new("settings-layer"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(96));
                let dim = ui.interact(screen, egui::Id::new("settings-dim"), egui::Sense::click());
                if dim.clicked() {
                    cancel = true;
                }
                let card = egui::Rect::from_center_size(screen.center(), size);
                ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                    egui::Frame::NONE
                        .fill(p.card)
                        .corner_radius(CornerRadius::same(12))
                        .stroke(Stroke::new(1.0, p.line))
                        .shadow(theme::card_shadow(&p))
                        .inner_margin(Margin::same(0))
                        .show(ui, |ui| {
                            ui.set_min_size(size);
                            ui.set_max_size(size);
                            header(ui, &p, &mut cancel);
                            ui.separator();
                            let footer_h = 64.0;
                            let body_h = (size.y - 56.0 - footer_h).max(200.0);
                            ui.horizontal(|ui| {
                                ui.set_min_height(body_h);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(196.0, body_h),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.add_space(8.0);
                                        nav(ui, &mut self.settings.tab);
                                    },
                                );
                                ui.separator();
                                ui.vertical(|ui| {
                                    ui.set_width((size.x - 210.0).max(200.0));
                                    egui::ScrollArea::vertical()
                                        .id_salt("settings-body")
                                        .max_height(body_h)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.add_space(8.0);
                                            ui.set_min_width(ui.available_width());
                                            match self.settings.tab {
                                                Tab::General => general_tab(ui, &mut self.settings),
                                                Tab::Security => {
                                                    self.app_lock_settings(ui);
                                                    security_tab(
                                                        ui,
                                                        &mut self.settings,
                                                        self.scm_installed,
                                                        self.scm_running,
                                                        self.scm_known,
                                                    );
                                                }
                                                Tab::Privacy => self.privacy_tab(ui),
                                                Tab::Network => self.network_tab(ui),
                                                Tab::Display => display_tab(ui, &mut self.settings),
                                                Tab::Account => self.account_tab(ui),
                                                Tab::About => about_tab(ui),
                                            }
                                            ui.add_space(12.0);
                                        });
                                });
                            });
                            ui.separator();
                            footer(ui, &mut save, &mut cancel);
                        });
                });
            });
        let _ = layer;

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if cancel && !save {
            theme::apply(ctx, &self.cfg.settings.theme);
            self.show_settings = false;
        }
        // Enter in a Settings password field saves, like the Save button.
        save |= self.ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(SAVE_ON_ENTER)).unwrap_or(false));
        if save && self.apply_settings() {
            self.show_settings = false;
        }
    }

    fn privacy_tab(&mut self, ui: &mut egui::Ui) {
        widgets::heading(ui, "PRIVACY");
        ui.label("Saved computers on this computer");
        ui.label(widgets::muted(
            ui,
            "Removing them here does not delete them from your pjdesk.com account or from your other computers.",
        ));
        ui.add_space(8.0);
        if ui.button("Remove all saved computers from this computer").clicked() {
            self.confirm_clear_saved = true;
        }
        ui.add_space(8.0);
        if ui.button("Sign out and remove everything from this computer").clicked() {
            self.confirm_sign_out_clear = true;
        }
        ui.add_space(8.0);
        ui.label(widgets::muted(
            ui,
            "Sign out and remove also ends the pjdesk.com sign-in on this computer. When Sign out when PJDesk closes is on, closing PJDesk clears the saved list, cached passwords, and recent computers.",
        ));
    }

    fn account_tab(&mut self, ui: &mut egui::Ui) {
        widgets::heading(ui, "PJDESK ACCOUNT");
        let email = self.cfg.network_email.clone().unwrap_or_default();
        if email.trim().is_empty() {
            ui.label("Sign in with your pjdesk.com account.");
            ui.add_space(8.0);
            if ui.button("Sign in").clicked() {
                self.show_net_sign_in = true;
                self.show_settings = false;
            }
        } else {
            ui.label(RichText::new(email.trim()).strong().size(16.0));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Manage account").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab("https://pjdesk.com/account"));
                }
                if ui.button("Sign out").clicked() {
                    self.sign_out_network();
                }
            });
            ui.add_space(12.0);
            ui.label("This sign-in");
            let until_close = self.temp.is_some() || self.cfg.ephemeral_network;
            if ui.selectable_label(!until_close, "Stay signed in on this computer").clicked() && until_close {
                self.stay_signed_in();
            }
            if ui.selectable_label(until_close, "Sign out when PJDesk closes").clicked() && !until_close {
                self.sign_out_when_closes();
            }
            ui.label(widgets::muted(
                ui,
                "Stay signed in keeps this account on this computer. The other choice ends the sign-in when you close PJDesk.",
            ));
        }
        if self.signed_in() && !self.cfg.syncs_saved_computers() {
            ui.add_space(8.0);
            if ui.button("Load my saved computers").clicked() {
                self.load_saved_computers();
            }
            ui.label(widgets::muted(
                ui,
                "This downloads your saved computers and their passwords onto this computer, then keeps them in sync.",
            ));
        }
        ui.add_space(8.0);
        ui.label(widgets::muted(
            ui,
            "The account is managed at pjdesk.com. Passwords for individual computers stay on this computer.",
        ));
    }

    /// Applies the draft. Returns false when a field was rejected and the
    /// window should stay open.
    fn apply_settings(&mut self) -> bool {
        let d = &self.settings;

        let server_text = d.server.trim().to_string();
        let new_server = if server_text.is_empty() {
            None
        } else {
            match ServerAddr::parse(&server_text) {
                Ok(addr) => Some(addr.to_string()),
                Err(e) => {
                    self.status = format!("Server address: {e}");
                    return false;
                }
            }
        };

        if d.use_permanent && d.permanent_password.trim().chars().count() < 8 {
            self.status = "Use at least 8 characters for a permanent password.".into();
            return false;
        }

        let ports = match parse_ports(&d.tunnel_ports) {
            Ok(ports) => ports,
            Err(e) => {
                self.status = e;
                return false;
            }
        };

        let key_text = d.access_key.trim().to_string();
        if !key_text.is_empty() {
            match pjdesk_proto::netkey::classify_paste(&key_text) {
                pjdesk_proto::netkey::PasteKind::Network => {
                    self.status = pjdesk_proto::netkey::NETWORK_KEY_GOES.into();
                    return false;
                }
                pjdesk_proto::netkey::PasteKind::Access => {}
                pjdesk_proto::netkey::PasteKind::AccessIncomplete => {
                    self.status = pjdesk_proto::netkey::ACCESS_KEY_INCOMPLETE.into();
                    return false;
                }
                pjdesk_proto::netkey::PasteKind::Neither => {
                    self.status = "That is not a server access key. A server access key starts with pjdesk-key1. A network key starts with pjdesk-net1 and goes in Use a network key.".into();
                    return false;
                }
            }
        }
        let key_changed = key_text != self.cfg.server_access_key();
        let server_changed = self.cfg.server != new_server;
        if server_changed {
            self.cfg.server_fingerprint = None;
        }
        self.cfg.server = new_server;
        if key_changed {
            self.cfg.set_server_access_key(&key_text);
        }

        let old = self.cfg.settings.clone();
        let s = &mut self.cfg.settings;
        s.start_with_windows = d.start_with_windows;
        s.remember_passwords = d.remember_passwords;
        s.temp_idle_minutes = d.temp_idle_minutes.clamp(1, 24 * 60);
        s.view_only = d.view_only;
        s.fps = d.fps.clamp(1, 120);
        s.bitrate_kbps = d.bitrate_kbps.clamp(200, 50_000);
        s.max_width = d.max_width.clamp(640, 7680);
        s.force_gdi = d.force_gdi;
        s.theme = normalize_theme(&d.theme);
        s.approval = if d.approval == "unattended" { "unattended".into() } else { "prompt".into() };
        s.receive_dir = d.receive_dir.trim().to_string();
        s.allow_direct = d.allow_direct;
        s.discover_lan = d.discover_lan;
        s.tunnel_ports = ports;
        if d.use_permanent {
            s.set_permanent_password(Some(d.permanent_password.trim()));
        } else {
            s.set_permanent_password(None);
        }
        let theme_name = s.theme.clone();

        let host_changed = old.fps != s.fps
            || old.bitrate_kbps != s.bitrate_kbps
            || old.max_width != s.max_width
            || old.force_gdi != s.force_gdi
            || old.view_only != s.view_only
            || old.permanent_password != s.permanent_password
            || old.approval != s.approval
            || old.receive_dir != s.receive_dir
            || old.allow_direct != s.allow_direct
            || old.discover_lan != s.discover_lan
            || old.tunnel_ports != s.tunnel_ports;
        theme::apply(&self.egui_ctx(), &theme_name);

        if old.start_with_windows != d.start_with_windows {
            if let Err(e) = set_run_at_login(d.start_with_windows) {
                self.status = format!("Could not change Windows startup: {e}");
            }
        }

        // Compare against the cached task, never a fresh schtasks query.
        let observed = self.probe.as_ref().and_then(|p| p.latest.update_task);
        let update_changed = match observed {
            Some(on) => on != d.auto_update && (d.auto_update_known || d.auto_update_touched),
            None => d.auto_update_touched,
        };
        if update_changed {
            let args: &[&str] = if d.auto_update { &["update-task"] } else { &["update-task", "--disable"] };
            crate::platform::run_elevated(args);
            self.toast.show("Approve the administrator prompt to change automatic updates");
        }

        if let Err(e) = crate::hostfacts::write_computer_name(d.computer_name.trim()) {
            self.status = format!("Could not save this computer's name: {e}");
        }

        self.save_cfg();
        if key_changed {
            crate::network::copy_to_service(&self.cfg);
        }

        if server_changed && self.scm_known && self.scm_installed {
            if let Some(addr) = self.cfg.server.clone() {
                crate::platform::run_elevated(&["service", "configure", "--server", &addr]);
            }
        }

        crate::host::direct::set_lan_discovery(self.cfg.settings.discover_lan);
        self.ensure_lan_watch();
        if server_changed || host_changed || key_changed {
            let running = if self.scm_known { self.scm_running } else { self.engine.service_running };
            let cfg = self.cfg.clone();
            self.engine.restart(&cfg, running);
        }
        self.toast.show("Settings saved");
        true
    }

    fn egui_ctx(&self) -> egui::Context {
        // Settings is only drawn from a frame, which has stored the context.
        self.ctx.clone()
    }
}

fn header(ui: &mut egui::Ui, p: &theme::Paint, cancel: &mut bool) {
    egui::Frame::NONE.inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Settings").strong().size(18.0).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, super::icons::Icon::Close, "Close").clicked() {
                    *cancel = true;
                }
            });
        });
    });
}

const SAVE_ON_ENTER: &str = "pjdesk-settings-save-on-enter";

/// Enter in this Settings field saves the dialog at the end of the frame.
fn save_on_enter(ui: &egui::Ui, response: &egui::Response) {
    if widgets::submitted(ui, response) {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(SAVE_ON_ENTER), true));
    }
}

fn footer(ui: &mut egui::Ui, save: &mut bool, cancel: &mut bool) {
    let p = theme::paint(ui);
    egui::Frame::NONE.inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    [120.0, 36.0],
                    egui::Button::new(RichText::new("Save").strong().color(p.on_accent)).fill(p.accent),
                )
                .clicked()
            {
                *save = true;
            }
            if ui.add_sized([120.0, 36.0], egui::Button::new("Cancel")).clicked() {
                *cancel = true;
            }
        });
    });
}

fn nav(ui: &mut egui::Ui, tab: &mut Tab) {
    ui.set_width(180.0);
    for (which, label) in [
        (Tab::General, "General"),
        (Tab::Security, "Security"),
        (Tab::Privacy, "Privacy"),
        (Tab::Network, "Network"),
        (Tab::Display, "Display"),
        (Tab::Account, "Account"),
        (Tab::About, "About"),
    ] {
        let selected = *tab == which;
        let p = theme::paint(ui);
        let text =
            if selected { RichText::new(label).strong().color(p.text) } else { RichText::new(label).color(p.muted) };
        let fill = if selected { p.accent.linear_multiply(0.18) } else { Color32::TRANSPARENT };
        if ui.add_sized([ui.available_width(), 36.0], egui::Button::new(text).fill(fill).frame(selected)).clicked() {
            *tab = which;
        }
    }
}

fn general_tab(ui: &mut egui::Ui, d: &mut SettingsDraft) {
    widgets::heading(ui, "THIS COMPUTER");
    ui.label("This computer's name");
    widgets::text_field(ui, &mut d.computer_name, "Leave blank to use the Windows name", false);
    ui.label(widgets::muted(
        ui,
        "Shown in Saved computers on every signed-in computer. A nickname typed in Saved computers replaces it on that screen only.",
    ));
    ui.add_space(12.0);
    widgets::heading(ui, "APPEARANCE");
    ui.label(widgets::muted(ui, "Match Windows follows the Windows app theme."));
    let before = d.theme.clone();
    widgets::radio(ui, &mut d.theme, "system", "Match Windows");
    widgets::radio(ui, &mut d.theme, "dark", "Dark");
    widgets::radio(ui, &mut d.theme, "light", "Light");
    if d.theme != before {
        theme::apply(ui.ctx(), &d.theme);
    }

    ui.add_space(12.0);
    widgets::heading(ui, "STARTUP");
    widgets::checkbox(ui, &mut d.start_with_windows, "Start PJDesk when I sign in to Windows");
    widgets::checkbox(ui, &mut d.remember_passwords, "Remember passwords for new computers");
    ui.label(widgets::muted(
        ui,
        "When this is on, the password prompt starts with Remember password checked. The password is saved only after the other computer accepts it.",
    ));
    ui.add_space(8.0);
    ui.label("Sign out a temporary sign-in after this many minutes with no PJDesk activity");
    ui.add(egui::DragValue::new(&mut d.temp_idle_minutes).range(1..=24 * 60).suffix(" min"));
    ui.label(widgets::muted(
        ui,
        "For always-on access (even at the login screen), install the background service on the Security page.",
    ));

    ui.add_space(12.0);
    widgets::heading(ui, "UPDATES");
    ui.horizontal(|ui| {
        if !d.auto_update_known && !d.auto_update_touched {
            widgets::spinner(ui, theme::paint(ui).muted);
            ui.label(widgets::muted(ui, "Checking automatic updates…"));
        }
    });
    if widgets::checkbox(ui, &mut d.auto_update, "Install updates automatically (checks every 30 minutes)") {
        d.auto_update_touched = true;
        d.auto_update_known = true;
    }
    ui.label(widgets::muted(
        ui,
        "The background service checks every 30 minutes and installs the update itself. PJDesk does not register a scheduled task. Changing this asks for Administrator rights.",
    ));
    ui.add_space(8.0);
    update_now_button(ui);
}

fn security_tab(ui: &mut egui::Ui, d: &mut SettingsDraft, installed: bool, running: bool, known: bool) {
    widgets::heading(ui, "UNATTENDED ACCESS");
    widgets::checkbox(ui, &mut d.use_permanent, "Set a permanent password for this computer");
    ui.add_enabled_ui(d.use_permanent, |ui| {
        let r = widgets::password_field(ui, &mut d.permanent_password, widgets::PERMANENT_PASSWORD_HINT, &mut d.show_permanent);
        save_on_enter(ui, &r);
    });
    ui.label(widgets::muted(
        ui,
        "At least 8 characters. With a permanent password, you can connect to this computer any time without someone here reading out a one-time code. This is not your PJDesk account password.",
    ));

    ui.add_space(12.0);
    widgets::heading(ui, "WHEN SOMEONE CONNECTS");
    widgets::radio(ui, &mut d.approval, "prompt", "Ask the person at this computer first");
    widgets::radio(ui, &mut d.approval, "unattended", "Accept on the password alone (unattended)");
    ui.add_space(4.0);
    widgets::checkbox(ui, &mut d.view_only, "View only by default (don't let them use the mouse and keyboard)");

    ui.add_space(12.0);
    widgets::heading(ui, "INCOMING FILES");
    widgets::field_label(ui, "Folder for files people send you");
    widgets::text_field(ui, &mut d.receive_dir, "Empty uses this computer's Desktop", false);
    ui.label(widgets::muted(
        ui,
        "Files dropped on a remote session are saved here. Leave this empty to use the Desktop. Type Downloads to use that folder. Files are never executed.",
    ));

    ui.add_space(12.0);
    widgets::heading(ui, "BACKGROUND SERVICE");
    let p = theme::paint(ui);
    if !known {
        ui.horizontal(|ui| {
            widgets::spinner(ui, p.muted);
            ui.label(widgets::muted(ui, "Checking the background service…"));
        });
    } else if installed {
        widgets::status_dot(
            ui,
            if running { p.ok } else { p.warn },
            if running { "Installed and running" } else { "Installed, not running" },
        );
    } else {
        widgets::status_dot(ui, p.muted, "Not installed");
    }
    ui.label(widgets::muted(
        ui,
        "The service lets you reach this computer before anyone signs in and during UAC prompts. Each button asks for Administrator rights.",
    ));
    ui.add_space(4.0);
    if known {
        ui.horizontal(|ui| {
            if !installed {
                if ui.add(egui::Button::new("Install service").fill(theme::paint(ui).accent)).clicked() {
                    d.service_note = "Approve the administrator prompt to install the service.".into();
                    crate::platform::run_elevated(&["service", "install", "--firewall"]);
                }
            } else {
                if ui.button("Uninstall service").clicked() {
                    d.service_note = "Approve the administrator prompt to uninstall the service.".into();
                    crate::platform::run_elevated(&["service", "uninstall"]);
                }
                if running {
                    if ui.button("Stop").clicked() {
                        d.service_note = "Approve the administrator prompt to stop the service.".into();
                        crate::platform::run_elevated(&["service", "stop"]);
                    }
                } else if ui.button("Start").clicked() {
                    d.service_note = "Approve the administrator prompt to start the service.".into();
                    crate::platform::run_elevated(&["service", "start"]);
                }
            }
        });
    }
    if known && installed {
        ui.add_space(6.0);
        ui.label(widgets::muted(ui, "Set the service's unattended password"));
        ui.horizontal(|ui| {
            let r = widgets::password_field(
                ui,
                &mut d.service_password,
                widgets::PERMANENT_PASSWORD_HINT,
                &mut d.show_service_password,
            );
            let enter = widgets::submitted(ui, &r);
            if (ui.button("Apply").clicked() || enter) && !d.service_password.trim().is_empty() {
                let pw = d.service_password.trim().to_string();
                if pw.chars().count() < 8 {
                    d.service_note = "Use at least 8 characters.".into();
                    return;
                }
                d.service_note = "Approve the administrator prompt to set the service password.".into();
                crate::platform::run_elevated_with_password(&["service", "configure"], &pw);
                d.service_password.clear();
            }
        });
    }
    if !d.service_note.is_empty() {
        ui.label(RichText::new(&d.service_note).color(theme::paint(ui).warn).size(12.5));
    }
}

impl App {
    fn network_tab(&mut self, ui: &mut egui::Ui) {
        self.poll_access_keys();
        let mut do_import = false;
        let mut do_export = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("ID/RELAY SERVER").color(theme::paint(ui).muted).size(11.5).strong());
            ui.add_space(8.0);
            if ui
                .button("Import server config")
                .on_hover_text("Read a network key from the clipboard and apply it")
                .clicked()
            {
                do_import = true;
            }
            if ui.button("Export server config").on_hover_text("Copy this computer's network key").clicked() {
                do_export = true;
            }
        });
        ui.add_space(4.0);
        widgets::field_label(ui, "Address (host or host:port)");
        widgets::text_field(ui, &mut self.settings.server, "desk.example.com  or  192.168.1.10", false);
        ui.add_space(6.0);
        widgets::field_label(ui, "Access key");
        let r = widgets::password_field(ui, &mut self.settings.access_key, "pjdesk-key1.…", &mut self.settings.show_access_key);
        save_on_enter(ui, &r);
        ui.label(widgets::muted(
            ui,
            "From the person who runs the server. With this key, this computer can register when signed-in accounts are required.",
        ));
        let pinned = self.cfg.server_fingerprint.clone().unwrap_or_default();
        ui.label(widgets::muted(
            ui,
            if pinned.is_empty() {
                "Filled in when you import a server config or sign in. The default port is 21200."
            } else {
                "The pinned key is saved with the server. The default port is 21200."
            },
        ));
        if !pinned.is_empty() {
            ui.add_space(6.0);
            widgets::field_label(ui, "Key");
            ui.label(RichText::new(&pinned).monospace().size(13.0));
        }
        ui.add_space(6.0);
        ui.label(widgets::muted(
            ui,
            "Import reads a network key from the clipboard and applies it. Export copies this computer's network key. The text is encrypted, so it does not show the server address.",
        ));

        ui.add_space(12.0);
        self.owner_access_keys(ui);
        ui.add_space(12.0);
        self.enter_support_key(ui);
        ui.add_space(12.0);
        self.support_keys_editor(ui);
        ui.add_space(12.0);
        widgets::heading(ui, "DIRECT CONNECTIONS");
        widgets::checkbox(ui, &mut self.settings.allow_direct, "Allow direct IP connections");
        ui.add_space(6.0);
        widgets::checkbox(
            ui,
            &mut self.settings.discover_lan,
            "Let other computers on this network find this one",
        );
        ui.label(widgets::muted(
            ui,
            "On unless you uncheck it. PJDesk only answers on a private network. The announcement is the name and ID, never a password. Direct connections must also be allowed for someone to connect.",
        ));
        ui.add_space(8.0);
        widgets::field_label(ui, "Ports a remote session may forward");
        widgets::text_field(ui, &mut self.settings.tunnel_ports, "22, 3389", false);
        ui.label(widgets::muted(
            ui,
            "Comma-separated. Empty means no ports. Forwarding stays on this computer unless the server policy says otherwise.",
        ));
        if do_import {
            self.import_server_config();
        }
        if do_export {
            self.export_server_config();
        }
    }

    fn poll_access_keys(&mut self) {
        let Some(rx) = &self.access_panel.rx else { return };
        let Ok(job) = rx.try_recv() else { return };
        self.access_panel.rx = None;
        self.access_panel.busy = false;
        match job {
            crate::access_keys::Job::Ready(reply) => {
                self.access_panel.keys = reply.keys;
                self.access_panel.loaded = true;
                self.access_panel.hidden = false;
                if self.cfg.network_role != "owner" {
                    self.cfg.network_role = "owner".into();
                    self.save_cfg();
                }
                if !reply.secret.is_empty() {
                    self.access_panel.secret = reply.secret;
                    self.access_panel.show = true;
                    self.access_panel.label.clear();
                    self.access_panel.note = "Copy this key now. It is shown once.".into();
                } else if !reply.message.is_empty() {
                    self.access_panel.note = reply.message;
                } else {
                    self.access_panel.note.clear();
                }
            }
            crate::access_keys::Job::Failed(message) => {
                self.access_panel.loaded = true;
                if message.contains("Only the account") {
                    self.access_panel.hidden = true;
                    if self.cfg.network_role.is_empty() {
                        self.cfg.network_role = "member".into();
                        self.save_cfg();
                    }
                }
                self.access_panel.note = message;
            }
        }
    }

    fn owner_access_keys(&mut self, ui: &mut egui::Ui) {
        let signed_in = self.cfg.network_email.as_deref().map(str::trim).is_some_and(|email| !email.is_empty());
        if !signed_in || self.cfg.network_role == "member" || self.access_panel.hidden {
            return;
        }
        if !self.access_panel.loaded && self.access_panel.rx.is_none() {
            self.spawn_access_keys("list", String::new());
        }
        if self.cfg.network_role != "owner" && !self.access_panel.loaded {
            return;
        }
        if self.access_panel.busy || self.access_panel.rx.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        widgets::heading(ui, "SERVER ACCESS KEYS");
        ui.label(widgets::muted(
            ui,
            "A key lets someone use your server without signing in to your account. It is shown once. The server stores only a hash.",
        ));
        ui.add_space(6.0);
        if !self.access_panel.secret.is_empty() {
            ui.label(widgets::muted(ui, "Copy this key now. It is shown once and is not saved here."));
            let mut shown = self.access_panel.secret.clone();
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .desired_rows(2)
                    .desired_width(420.0)
                    .font(egui::TextStyle::Monospace)
                    .password(!self.access_panel.show)
                    .interactive(false),
            );
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    widgets::copy_to_clipboard(&self.access_panel.secret);
                    self.access_panel.note = "Access key copied.".into();
                }
                let toggle = if self.access_panel.show { "Hide" } else { "Show" };
                if ui.button(toggle).clicked() {
                    self.access_panel.show = !self.access_panel.show;
                }
                if ui.button("Done").clicked() {
                    self.access_panel.secret.clear();
                    self.access_panel.show = false;
                }
            });
            ui.add_space(6.0);
        }
        widgets::field_label(ui, "Label");
        widgets::text_field(ui, &mut self.access_panel.label, "Ada", false);
        ui.label(widgets::muted(ui, "Expires"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.access_panel.days, 0, "Never");
            ui.selectable_value(&mut self.access_panel.days, 1, "1 day");
            ui.selectable_value(&mut self.access_panel.days, 7, "7 days");
            ui.selectable_value(&mut self.access_panel.days, 30, "30 days");
        });
        widgets::field_label(ui, "Computer limit");
        widgets::text_field(ui, &mut self.access_panel.limit, "1", false);
        if self.access_panel.limit.is_empty() {
            self.access_panel.limit = "1".into();
        }
        let busy = self.access_panel.busy;
        if ui.add_enabled(!busy, egui::Button::new(if busy { "Creating…" } else { "Create key" })).clicked() {
            let label = self.access_panel.label.trim().to_string();
            if label.is_empty() {
                self.access_panel.note = "Give the key a label, such as the person's name.".into();
            } else {
                self.spawn_access_keys("create", String::new());
            }
        }
        if !self.access_panel.note.is_empty() {
            ui.label(widgets::muted(ui, &self.access_panel.note));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let keys = self.access_panel.keys.clone();
        for key in keys {
            let expired = key.revoked_at == 0 && key.expires_at != 0 && now >= key.expires_at;
            if key.revoked_at != 0 || expired {
                continue;
            }
            let when = if key.expires_at == 0 { "does not expire" } else { "expires" };
            ui.add_space(4.0);
            ui.label(RichText::new(&key.label).strong());
            ui.label(widgets::muted(
                ui,
                &format!("{when} · limit {} · {} computer(s)", key.computer_limit.max(1), key.computers),
            ));
            if !key.names.is_empty() {
                ui.label(widgets::muted(ui, &key.names.join(", ")));
            }
            let confirming = self.access_panel.confirm.as_deref() == Some(key.id.as_str());
            ui.horizontal(|ui| {
                if confirming {
                    ui.label("Revoke this key? Every computer that used it will be disconnected and removed.");
                    if ui.small_button("Revoke").clicked() {
                        let id = key.id.clone();
                        self.access_panel.confirm = None;
                        self.spawn_access_keys("revoke", id);
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.access_panel.confirm = None;
                    }
                } else if ui.small_button("Revoke").clicked() {
                    self.access_panel.confirm = Some(key.id.clone());
                }
            });
        }
        ui.add_space(8.0);
    }

    fn spawn_access_keys(&mut self, op: &str, key_id: String) {
        if self.access_panel.busy {
            return;
        }
        let Some(server) = self.server_addr() else {
            self.access_panel.note = "Set the server address first.".into();
            return;
        };
        let limit = if op == "create" {
            match crate::access_keys::parse_computer_limit(&self.access_panel.limit) {
                Ok(n) => n,
                Err(message) => {
                    self.access_panel.note = message.into();
                    return;
                }
            }
        } else {
            1
        };
        let label = self.access_panel.label.clone();
        let expires = crate::access_keys::expires_secs(self.access_panel.days);
        let mut cfg = self.cfg.clone();
        let op = op.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.access_panel.busy = true;
        self.access_panel.rx = Some(rx);
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(crate::access_keys::Job::Failed(e.to_string()));
                    return;
                }
            };
            let job = match rt.block_on(crate::access_keys::manage(&server, &mut cfg, &op, &label, expires, limit, &key_id)) {
                Ok(reply) => crate::access_keys::Job::Ready(reply),
                Err(e) => crate::access_keys::Job::Failed(e.to_string()),
            };
            let _ = tx.send(job);
        });
    }

    pub(super) fn support_keys_window(&mut self, ctx: &egui::Context) {
        if !self.show_support_keys {
            return;
        }
        let mut open = true;
        egui::Window::new("Support keys")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                self.support_keys_editor(ui);
            });
        if !open {
            self.show_support_keys = false;
        }
    }

    fn enter_support_key(&mut self, ui: &mut egui::Ui) {
        widgets::heading(ui, "ENTER SUPPORT KEY");
        ui.label(widgets::muted(
            ui,
            "From the person supporting this computer. No PJDesk account is required. The key is not saved on this computer.",
        ));
        ui.add_space(6.0);
        widgets::field_label(ui, "Support key");
        let key_box =
            widgets::password_field(ui, &mut self.support_keys.enter_code, "1234 5678", &mut self.support_keys.enter_show);
        let key_enter = widgets::submitted(ui, &key_box);
        if self.support_keys.need_consent {
            ui.add_space(6.0);
            let owner = self.support_keys.enter_owner.clone();
            let allow = format!("I allow {owner} to connect to this computer without asking each time.");
            widgets::checkbox(ui, &mut self.support_keys.enter_consent, &allow);
            if !self.support_keys.enter_label.is_empty() {
                ui.label(widgets::muted(ui, &format!("This key is labeled {}.", self.support_keys.enter_label)));
            }
        }
        let busy = self.net_busy;
        if ui.add_enabled(!busy, egui::Button::new(if busy { "Checking…" } else { "Use this key" })).clicked() || (key_enter && !busy) {
            self.submit_support_key();
        }
        if !self.support_keys.enter_note.is_empty() && !self.support_keys.need_consent {
            ui.label(widgets::muted(ui, &self.support_keys.enter_note));
        } else if self.support_keys.need_consent && self.support_keys.enter_note.contains("Tick") {
            ui.label(widgets::muted(ui, &self.support_keys.enter_note));
        } else if self.support_keys.need_consent {
            ui.label(widgets::muted(ui, &self.support_keys.enter_note));
        }
    }

    fn submit_support_key(&mut self) {
        let digits: String = self.support_keys.enter_code.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() != 8 {
            self.support_keys.enter_note = "Enter the 8-digit support key.".into();
            return;
        }
        if self.support_keys.need_consent && !self.support_keys.enter_consent {
            self.support_keys.enter_note = "Tick the box to allow unattended access.".into();
            return;
        }
        self.net_mode = 1;
        self.net_code = digits;
        self.net_consent = self.support_keys.enter_consent;
        self.net_temporary = false;
        self.net_enroll_only = false;
        self.support_keys.enter_note.clear();
        self.start_network_sign_in();
    }

    fn support_keys_editor(&mut self, ui: &mut egui::Ui) {
        let signed_in = self.cfg.network_email.as_deref().map(str::trim).is_some_and(|email| !email.is_empty());
        if !signed_in || !self.cfg.can_manage_support_keys() {
            if !signed_in && self.cfg.support_grant.is_none() {
                ui.label(widgets::muted(ui, "Sign in to create a support key."));
            }
            return;
        }
        widgets::heading(ui, "SUPPORT KEYS");
        ui.label(widgets::muted(
            ui,
            "A support key lets a client join your network without an account. It is shown once. Only a hash is stored. This is separate from a server access key.",
        ));
        ui.add_space(6.0);
        if !self.support_keys.loaded && self.support_keys.rx.is_none() {
            self.spawn_support_keys("list", String::new(), String::new(), String::new());
        }
        if self.support_keys.busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !self.support_keys.secret.is_empty() {
            ui.label(widgets::muted(ui, "Copy this key now. It is shown once."));
            let shown = crate::network::group_digits(&self.support_keys.secret);
            ui.label(RichText::new(shown).size(28.0).strong().monospace());
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    widgets::copy_to_clipboard(&crate::network::group_digits(&self.support_keys.secret));
                    self.support_keys.note = "Support key copied.".into();
                }
                if ui.button("Done").clicked() {
                    self.support_keys.secret.clear();
                    self.support_keys.show = false;
                }
            });
            ui.add_space(6.0);
        }
        widgets::field_label(ui, "Label");
        widgets::text_field(ui, &mut self.support_keys.label, "Client or company", false);
        ui.label(widgets::muted(ui, "Use"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.support_keys.reusable, false, "Single-use");
            ui.selectable_value(&mut self.support_keys.reusable, true, "Reusable");
        });
        ui.label(widgets::muted(ui, "Expires"));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.support_keys.expiry, 3600, "1 hour");
            ui.selectable_value(&mut self.support_keys.expiry, 86_400, "1 day");
            ui.selectable_value(&mut self.support_keys.expiry, 604_800, "7 days");
            ui.selectable_value(&mut self.support_keys.expiry, 2_592_000, "30 days");
            ui.selectable_value(&mut self.support_keys.expiry, 0, "Never");
        });
        widgets::field_label(ui, "Computer limit");
        widgets::text_field(ui, &mut self.support_keys.limit, "1", false);
        if self.support_keys.limit.is_empty() {
            self.support_keys.limit = "1".into();
        }
        ui.label(widgets::muted(ui, "Connecting"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.support_keys.unattended, false, "Client clicks Allow");
            ui.selectable_value(&mut self.support_keys.unattended, true, "Unattended");
        });
        ui.label(widgets::muted(
            ui,
            "Unattended opens the computer without an Allow click. The client still ticks a consent box when they enter the key.",
        ));
        let busy = self.support_keys.busy;
        if ui.add_enabled(!busy, egui::Button::new(if busy { "Creating…" } else { "Create" })).clicked() {
            if self.support_keys.label.trim().is_empty() {
                self.support_keys.note = "Give the key a label, such as the client's name.".into();
            } else if !crate::support_keys::expiry_allowed(self.support_keys.expiry) {
                self.support_keys.note = "Choose 1 hour, 1 day, 7 days, 30 days, or never.".into();
            } else {
                self.spawn_support_keys("create", String::new(), String::new(), String::new());
            }
        }
        if !self.support_keys.note.is_empty() {
            ui.label(widgets::muted(ui, &self.support_keys.note));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let keys = self.support_keys.keys.clone();
        for key in keys {
            let expired = key.expires_at != 0 && now >= key.expires_at;
            if expired {
                continue;
            }
            let when = if key.expires_at == 0 {
                "does not expire".to_string()
            } else {
                format!("expires {}", crate::network::remaining_label(key.expires_at, now))
            };
            let kind = if key.reusable { "reusable" } else { "single-use" };
            let door = if key.unattended { "unattended" } else { "client clicks Allow" };
            ui.add_space(6.0);
            ui.label(RichText::new(&key.label).strong());
            ui.label(widgets::muted(
                ui,
                &format!("{kind} · {when} · limit {} · {door}", key.computer_limit.max(1)),
            ));
            let computers = key.computers.clone();
            for computer in computers {
                let who = if !computer.name.trim().is_empty() {
                    computer.name.clone()
                } else if !computer.pjdesk_id.is_empty() {
                    computer.pjdesk_id.clone()
                } else {
                    "A computer".into()
                };
                let mark_id = if computer.pjdesk_id.is_empty() { computer.device_id.clone() } else { computer.pjdesk_id.clone() };
                let mark = (key.id.clone(), mark_id);
                let confirming = self.support_keys.confirm_computer.as_ref() == Some(&mark);
                ui.horizontal(|ui| {
                    ui.label(who);
                    if confirming {
                        if ui.small_button("Remove").clicked() {
                            let id = key.id.clone();
                            let pjdesk = computer.pjdesk_id.clone();
                            let target = computer.device_id.clone();
                            self.support_keys.confirm_computer = None;
                            self.spawn_support_keys("remove", id, pjdesk, target);
                        }
                        if ui.small_button("Cancel").clicked() {
                            self.support_keys.confirm_computer = None;
                        }
                    } else if ui.small_button("Remove").clicked() {
                        self.support_keys.confirm_computer = Some(mark);
                    }
                });
            }
            let confirming = self.support_keys.confirm.as_deref() == Some(key.id.as_str());
            ui.horizontal(|ui| {
                if confirming {
                    ui.label("Revoke this key? Computers that used it lose access.");
                    if ui.small_button("Revoke").clicked() {
                        let id = key.id.clone();
                        self.support_keys.confirm = None;
                        self.spawn_support_keys("revoke", id, String::new(), String::new());
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.support_keys.confirm = None;
                    }
                } else if ui.small_button("Revoke").clicked() {
                    self.support_keys.confirm = Some(key.id.clone());
                }
            });
        }
    }

    pub(super) fn poll_support_keys(&mut self) {
        let Some(rx) = &self.support_keys.rx else { return };
        let Ok(job) = rx.try_recv() else { return };
        self.support_keys.rx = None;
        self.support_keys.busy = false;
        match job {
            crate::support_keys::Job::Created { code, keys } => {
                self.support_keys.secret = code;
                self.support_keys.keys = keys;
                self.support_keys.loaded = true;
                self.support_keys.label.clear();
                self.support_keys.note = "Copy this key now. It is shown once.".into();
            }
            crate::support_keys::Job::Listed(keys) => {
                self.support_keys.keys = keys;
                self.support_keys.loaded = true;
                if self.support_keys.note == "Working…" {
                    self.support_keys.note.clear();
                }
            }
            crate::support_keys::Job::Failed(message) => {
                self.support_keys.loaded = true;
                if message.contains("Only the") || message.contains("No network") {
                    self.support_keys.hidden = true;
                }
                self.support_keys.note = message;
            }
        }
    }

    pub(super) fn poll_support_leave(&mut self) {
        let Some(rx) = &self.support_keys.leave_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.support_keys.leave_rx = None;
        self.support_keys.leaving = false;
        match result {
            Ok(()) => self.finish_support_leave(),
            Err(message) => {
                if message.contains("Sign in required") || message.to_ascii_lowercase().contains("revoked") {
                    self.finish_support_leave();
                } else {
                    self.toast.show(message);
                }
            }
        }
    }

    fn finish_support_leave(&mut self) {
        self.cfg.leave_network_sign_in();
        let _ = self.cfg.save();
        crate::network::copy_to_service(&self.cfg);
        self.engine.retarget(&self.cfg);
        self.toast.show("Support access revoked");
    }

    pub(super) fn revoke_support_access(&mut self) {
        if self.support_keys.leaving || self.cfg.support_grant.is_none() {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            self.finish_support_leave();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.support_keys.leaving = true;
        self.support_keys.leave_rx = Some(rx);
        std::thread::spawn(move || {
            let result = crate::support_keys::leave(&refresh, &device).map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn spawn_support_keys(&mut self, op: &str, key_id: String, pjdesk_id: String, target_device: String) {
        if self.support_keys.busy {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            self.support_keys.note = "Sign in again.".into();
            return;
        }
        let limit = if op == "create" {
            match crate::access_keys::parse_computer_limit(&self.support_keys.limit) {
                Ok(n) => n,
                Err(message) => {
                    self.support_keys.note = message.into();
                    return;
                }
            }
        } else {
            1
        };
        let label = self.support_keys.label.trim().to_string();
        let reusable = self.support_keys.reusable;
        let expiry = self.support_keys.expiry;
        let unattended = self.support_keys.unattended;
        let op = op.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.support_keys.busy = true;
        self.support_keys.rx = Some(rx);
        if op == "create" {
            self.support_keys.note = "Working…".into();
        }
        std::thread::spawn(move || {
            let job = match op.as_str() {
                "create" => match crate::support_keys::create_key(&refresh, &device, &label, reusable, expiry, limit, unattended) {
                    Ok(created) => {
                        let keys = crate::support_keys::list_keys(&refresh, &device).unwrap_or_default();
                        crate::support_keys::Job::Created { code: created.code, keys }
                    }
                    Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                },
                "revoke" => match crate::support_keys::revoke_key(&refresh, &device, &key_id) {
                    Ok(()) => match crate::support_keys::list_keys(&refresh, &device) {
                        Ok(keys) => crate::support_keys::Job::Listed(keys),
                        Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                    },
                    Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                },
                "remove" => match crate::support_keys::remove_computer(&refresh, &device, &key_id, &pjdesk_id, &target_device) {
                    Ok(()) => match crate::support_keys::list_keys(&refresh, &device) {
                        Ok(keys) => crate::support_keys::Job::Listed(keys),
                        Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                    },
                    Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                },
                _ => match crate::support_keys::list_keys(&refresh, &device) {
                    Ok(keys) => crate::support_keys::Job::Listed(keys),
                    Err(e) => crate::support_keys::Job::Failed(e.to_string()),
                },
            };
            let _ = tx.send(job);
        });
    }
}

fn display_tab(ui: &mut egui::Ui, d: &mut SettingsDraft) {
    widgets::heading(ui, "QUALITY WHEN SHARING THIS SCREEN");
    egui::Grid::new("quality").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label("Max frame rate");
        ui.add(egui::Slider::new(&mut d.fps, 5..=60).suffix(" fps"));
        ui.end_row();
        ui.label("Bitrate");
        ui.add(egui::Slider::new(&mut d.bitrate_kbps, 500..=20_000).suffix(" kbit/s"));
        ui.end_row();
        ui.label("Downscale above");
        ui.add(egui::Slider::new(&mut d.max_width, 1280..=3840).suffix(" px wide"));
        ui.end_row();
    });
    ui.add_space(8.0);
    widgets::checkbox(ui, &mut d.force_gdi, "Use the compatibility screen grabber (GDI) instead of the fast one");
    ui.label(widgets::muted(ui, "Try this only if the fast grabber gives a black screen."));
}

fn about_tab(ui: &mut egui::Ui) {
    let p = theme::paint(ui);
    ui.add_space(4.0);
    ui.label(RichText::new("PJDesk").size(22.0).strong().color(p.text));
    ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).color(p.muted));
    ui.add_space(8.0);
    ui.label("A self-hosted, open-source remote desktop. Your computers connect only through the server you run.");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.hyperlink_to("pjdesk.com", "https://pjdesk.com");
        ui.label(RichText::new("·").color(p.muted));
        ui.hyperlink_to("Source on GitHub", "https://github.com/whoamib2/pjdesk");
    });
    ui.add_space(12.0);
    update_now_button(ui);
    ui.add_space(8.0);
    ui.label(widgets::muted(ui, "Keyboard: Enter connects, Ctrl+, opens Settings."));
}

/// Settings → General → Updates, and Settings → About. Also on the tray menu.
fn update_now_button(ui: &mut egui::Ui) {
    let note = crate::client_update::note();
    if !note.is_empty() {
        ui.label(RichText::new(&note).color(theme::paint(ui).text));
        ui.add_space(4.0);
    }
    let busy = crate::client_update::running();
    if ui.add_enabled(!busy, egui::Button::new("Check for updates now")).clicked() {
        crate::client_update::start();
    }
    ui.label(widgets::muted(
        ui,
        "Downloads the latest release, checks it, installs it, and opens PJDesk again.",
    ));
}

fn parse_ports(text: &str) -> Result<Vec<u16>, String> {
    let mut out = Vec::new();
    for part in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let port: u16 = part.parse().map_err(|_| format!("{part} is not a port number."))?;
        if port == 0 {
            return Err("Port 0 is not allowed.".into());
        }
        if !out.contains(&port) {
            out.push(port);
        }
    }
    Ok(out)
}

/// Adds or removes a per-user "Run at sign-in" entry for this executable.
#[cfg(windows)]
pub(crate) fn set_run_at_login(enable: bool) -> anyhow::Result<()> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
    };

    unsafe {
        let mut key = HKEY::default();
        let sub = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        RegOpenKeyExW(HKEY_CURRENT_USER, sub, Some(0), KEY_SET_VALUE, &mut key).ok()?;
        let name = w!("PJDesk");
        let result = if enable {
            let exe = std::env::current_exe()?;
            let value = HSTRING::from(format!("\"{}\" --tray", exe.display()));
            let bytes: &[u8] =
                std::slice::from_raw_parts(value.as_ptr() as *const u8, (value.len() + 1) * std::mem::size_of::<u16>());
            RegSetValueExW(key, name, Some(0), REG_SZ, Some(bytes)).ok()
        } else {
            let _ = RegDeleteValueW(key, name);
            Ok(())
        };
        let _ = RegCloseKey(key);
        let _ = PCWSTR::null();
        result?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn set_run_at_login(_enable: bool) -> anyhow::Result<()> {
    Ok(())
}
