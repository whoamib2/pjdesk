//! The settings window (General, Security, Network, Display, About).

use eframe::egui::{self, Color32, RichText};

use super::{theme, App};
use crate::config::Config;
use crate::signaling::ServerAddr;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    General,
    Security,
    Network,
    Display,
    About,
}

/// An editable copy of the settings, committed to the config only on Save.
pub struct SettingsDraft {
    tab: Tab,
    server: String,
    start_with_windows: bool,
    view_only: bool,
    use_permanent: bool,
    permanent_password: String,
    fps: u32,
    bitrate_kbps: u32,
    max_width: u32,
    force_gdi: bool,
    service_password: String,
}

impl SettingsDraft {
    pub fn from_config(cfg: &Config) -> Self {
        let s = &cfg.settings;
        Self {
            tab: Tab::General,
            server: cfg.server.clone().unwrap_or_default(),
            start_with_windows: s.start_with_windows,
            view_only: s.view_only,
            use_permanent: s.permanent_password.is_some(),
            permanent_password: s.permanent_password().unwrap_or_default(),
            fps: s.fps,
            bitrate_kbps: s.bitrate_kbps,
            max_width: s.max_width,
            force_gdi: s.force_gdi,
            service_password: String::new(),
        }
    }
}

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut cancel = false;

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::General, "General"),
                        (Tab::Security, "Security"),
                        (Tab::Network, "Network"),
                        (Tab::Display, "Display"),
                        (Tab::About, "About"),
                    ] {
                        ui.selectable_value(&mut self.settings.tab, tab, label);
                    }
                });
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| match self.settings.tab {
                    Tab::General => general_tab(&mut self.settings, ui),
                    Tab::Security => security_tab(&mut self.settings, ui),
                    Tab::Network => network_tab(&mut self.settings, ui),
                    Tab::Display => display_tab(&mut self.settings, ui),
                    Tab::About => about_tab(ui),
                });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Save").strong()).fill(theme::ACCENT)).clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel || !open {
            self.show_settings = false;
        }
        if save {
            self.apply_settings();
            self.show_settings = false;
        }
    }

    fn apply_settings(&mut self) {
        let d = &self.settings;

        // Validate the server address if one was entered.
        let server_text = d.server.trim().to_string();
        let new_server = if server_text.is_empty() {
            None
        } else {
            match ServerAddr::parse(&server_text) {
                Ok(addr) => Some(addr.to_string()),
                Err(e) => {
                    self.status = format!("Server address: {e}");
                    return;
                }
            }
        };

        let server_changed = self.cfg.server != new_server;
        if server_changed {
            // A different server has a different key.
            self.cfg.server_fingerprint = None;
        }
        self.cfg.server = new_server;

        let old = self.cfg.settings.clone();
        let s = &mut self.cfg.settings;
        s.start_with_windows = d.start_with_windows;
        s.view_only = d.view_only;
        s.fps = d.fps.clamp(1, 120);
        s.bitrate_kbps = d.bitrate_kbps.clamp(200, 50_000);
        s.max_width = d.max_width.clamp(640, 7680);
        s.force_gdi = d.force_gdi;
        if d.use_permanent {
            s.set_permanent_password(Some(d.permanent_password.trim()));
        } else {
            s.set_permanent_password(None);
        }

        let quality_changed = old.fps != s.fps
            || old.bitrate_kbps != s.bitrate_kbps
            || old.max_width != s.max_width
            || old.force_gdi != s.force_gdi
            || old.view_only != s.view_only
            || old.permanent_password != s.permanent_password;

        // Apply "start with Windows".
        if old.start_with_windows != d.start_with_windows {
            if let Err(e) = set_run_at_login(d.start_with_windows) {
                self.status = format!("Could not change Windows startup: {e}");
            }
        }

        self.save_cfg();

        if server_changed || quality_changed {
            self.engine.restart(&self.cfg);
        }
        self.toast.show("Settings saved");
    }
}

fn general_tab(d: &mut SettingsDraft, ui: &mut egui::Ui) {
    super::widgets::heading(ui, "STARTUP");
    ui.checkbox(&mut d.start_with_windows, "Start PJDesk when I sign in to Windows");
    ui.label(RichText::new("For always-on access (even at the login screen), install the background service — see the Security tab.").color(theme::MUTED).size(12.0));
}

fn security_tab(d: &mut SettingsDraft, ui: &mut egui::Ui) {
    super::widgets::heading(ui, "UNATTENDED ACCESS");
    ui.checkbox(&mut d.use_permanent, "Set a permanent password for this computer");
    ui.add_enabled_ui(d.use_permanent, |ui| {
        ui.horizontal(|ui| {
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut d.permanent_password).password(true).desired_width(200.0));
        });
    });
    ui.label(
        RichText::new(
            "With a permanent password, you can connect to this computer any time without someone here reading out a one-time code.",
        )
        .color(theme::MUTED)
        .size(12.0),
    );
    ui.add_space(10.0);
    super::widgets::heading(ui, "WHEN SOMEONE CONTROLS THIS COMPUTER");
    ui.checkbox(&mut d.view_only, "View only by default (don't let them use the mouse and keyboard)");
    ui.add_space(10.0);
    super::widgets::heading(ui, "BACKGROUND SERVICE");
    let (installed, running) = crate::service::status();
    if installed {
        super::widgets::status_dot(
            ui,
            if running { theme::OK } else { theme::WARN },
            if running { "Installed and running" } else { "Installed, not running" },
        );
    } else {
        super::widgets::status_dot(ui, theme::MUTED, "Not installed");
    }
    ui.label(
        RichText::new(
            "The service lets you reach this computer before anyone signs in and during UAC prompts. Each button asks for Administrator rights.",
        )
        .color(theme::MUTED)
        .size(12.0),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if !installed {
            if ui.add(egui::Button::new("Install service").fill(theme::ACCENT)).clicked() {
                crate::platform::run_elevated(&["service", "install", "--firewall"]);
            }
        } else {
            if ui.button("Uninstall service").clicked() {
                crate::platform::run_elevated(&["service", "uninstall"]);
            }
            if running {
                if ui.button("Stop").clicked() {
                    crate::platform::run_elevated(&["service", "stop"]);
                }
            } else if ui.button("Start").clicked() {
                crate::platform::run_elevated(&["service", "start"]);
            }
        }
    });
    if installed {
        ui.add_space(6.0);
        ui.label(RichText::new("Set the service's unattended password").color(theme::MUTED).size(12.0));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut d.service_password).password(true).desired_width(200.0));
            if ui.button("Apply").clicked() && !d.service_password.trim().is_empty() {
                crate::platform::run_elevated(&["service", "configure", "--password", d.service_password.trim()]);
                d.service_password.clear();
            }
        });
    }
}

fn network_tab(d: &mut SettingsDraft, ui: &mut egui::Ui) {
    super::widgets::heading(ui, "YOUR PJDESK SERVER");
    ui.label("Address (host or host:port)");
    ui.add(egui::TextEdit::singleline(&mut d.server).hint_text("desk.example.com  or  192.168.1.10").desired_width(f32::INFINITY));
    ui.label(
        RichText::new("This is the self-hosted PJDesk server your computers connect through. The default port is 21200.")
            .color(theme::MUTED)
            .size(12.0),
    );
}

fn display_tab(d: &mut SettingsDraft, ui: &mut egui::Ui) {
    super::widgets::heading(ui, "QUALITY WHEN SHARING THIS SCREEN");
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
    ui.checkbox(&mut d.force_gdi, "Use the compatibility screen grabber (GDI) instead of the fast one");
    ui.label(RichText::new("Try this only if the fast grabber gives a black screen.").color(theme::MUTED).size(12.0));
}

fn about_tab(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(RichText::new("PJDesk").size(20.0).strong().color(Color32::WHITE));
    ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).color(theme::MUTED));
    ui.add_space(8.0);
    ui.label("A self-hosted, open-source remote desktop. Your computers connect only through the server you run.");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.hyperlink_to("pjdesk.com", "https://pjdesk.com");
        ui.label(RichText::new("·").color(theme::MUTED));
        ui.hyperlink_to("Source on GitHub", "https://github.com/whoamib2/pjdesk");
    });
}

/// Adds or removes a per-user "Run at sign-in" entry for this executable.
#[cfg(windows)]
fn set_run_at_login(enable: bool) -> anyhow::Result<()> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_SZ,
    };

    unsafe {
        let mut key = HKEY::default();
        let sub = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        RegOpenKeyExW(HKEY_CURRENT_USER, sub, Some(0), KEY_SET_VALUE, &mut key).ok()?;
        let name = w!("PJDesk");
        let result = if enable {
            let exe = std::env::current_exe()?;
            let value = HSTRING::from(format!("\"{}\"", exe.display()));
            let bytes: &[u8] = std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                (value.len() + 1) * std::mem::size_of::<u16>(),
            );
            RegSetValueExW(key, name, Some(0), REG_SZ, Some(bytes)).ok()
        } else {
            // Removing a missing value is fine.
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
