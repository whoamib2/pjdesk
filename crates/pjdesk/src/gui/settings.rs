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
    tunnel_ports: String,
}

impl SettingsDraft {
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
            tunnel_ports: s.tunnel_ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "),
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
                                                Tab::Security => security_tab(
                                                    ui,
                                                    &mut self.settings,
                                                    self.scm_installed,
                                                    self.scm_running,
                                                    self.scm_known,
                                                ),
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
        if save && self.apply_settings() {
            self.show_settings = false;
        }
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

        let server_changed = self.cfg.server != new_server;
        if server_changed {
            self.cfg.server_fingerprint = None;
        }
        self.cfg.server = new_server;

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

        if server_changed && self.scm_known && self.scm_installed {
            if let Some(addr) = self.cfg.server.clone() {
                crate::platform::run_elevated(&["service", "configure", "--server", &addr]);
            }
        }

        if server_changed || host_changed {
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
            ui.label(widgets::muted(ui, "Checking the update schedule…"));
        }
    });
    if widgets::checkbox(ui, &mut d.auto_update, "Install updates automatically (checks every 30 minutes)") {
        d.auto_update_touched = true;
        d.auto_update_known = true;
    }
    ui.label(widgets::muted(
        ui,
        "Downloads new versions from the project's GitHub releases, verifies them, and installs silently. Changing this asks for Administrator rights.",
    ));
}

fn security_tab(ui: &mut egui::Ui, d: &mut SettingsDraft, installed: bool, running: bool, known: bool) {
    widgets::heading(ui, "UNATTENDED ACCESS");
    widgets::checkbox(ui, &mut d.use_permanent, "Set a permanent password for this computer");
    ui.add_enabled_ui(d.use_permanent, |ui| {
        widgets::password_field(ui, &mut d.permanent_password, widgets::PERMANENT_PASSWORD_HINT, &mut d.show_permanent);
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
            widgets::password_field(
                ui,
                &mut d.service_password,
                widgets::PERMANENT_PASSWORD_HINT,
                &mut d.show_service_password,
            );
            if ui.button("Apply").clicked() && !d.service_password.trim().is_empty() {
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
        widgets::heading(ui, "DIRECT CONNECTIONS");
        widgets::checkbox(
            ui,
            &mut self.settings.allow_direct,
            "Allow direct IP connections and announce this computer on the local network",
        );
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
    ui.label(widgets::muted(ui, "Keyboard: Enter connects, Ctrl+, opens Settings."));
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
fn set_run_at_login(enable: bool) -> anyhow::Result<()> {
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
            let value = HSTRING::from(format!("\"{}\"", exe.display()));
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
fn set_run_at_login(_enable: bool) -> anyhow::Result<()> {
    Ok(())
}
