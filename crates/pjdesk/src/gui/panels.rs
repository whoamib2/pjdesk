//! The main window: a left rail for this computer, and the remote desktop beside it.

use eframe::egui::{self, RichText, ScrollArea, Vec2};
use pjdesk_proto::signal::AddressEntry;
use pjdesk_proto::{format_id, normalize_id};

use super::icons::Icon;
use super::{theme, widgets, App};

/// How the main window divides the area under the top bar.
pub(crate) struct DeskSplit {
    pub stacked: bool,
    pub left: f32,
    pub list: f32,
}

/// `body_width` is the width of the main area, after the window margin.
pub(crate) fn desk_split(body_width: f32) -> DeskSplit {
    if body_width < 960.0 {
        DeskSplit { stacked: true, left: body_width, list: body_width }
    } else {
        let left = 400.0_f32.min((body_width * 0.34).max(340.0));
        let list = (body_width - left - 12.0).max(320.0);
        DeskSplit { stacked: false, left, list }
    }
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

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Gear, "Settings (Ctrl+,)").clicked() {
                    self.open_settings();
                }
                ui.separator();
                let email = self.cfg.network_email.clone().unwrap_or_default();
                let signed_in = self.temp.is_some() || !email.trim().is_empty();
                if signed_in && ui.button("Invite").on_hover_text("Make an 8-digit code someone can use to join your network.").clicked()
                {
                    self.open_invite();
                }
                if self.temp.is_some() {
                    ui.label(RichText::new("Signed in until you close PJDesk").color(p.warn).size(13.0));
                    if ui.button("Sign out").clicked() {
                        self.end_temporary_sign_in();
                    }
                } else if !email.trim().is_empty() {
                    ui.menu_button(RichText::new(email.trim()).color(p.ok), |ui| {
                        if ui.button("Manage account").clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab("https://pjdesk.com/account"));
                            ui.close();
                        }
                        if ui.button("Sign out").clicked() {
                            self.sign_out_network();
                            ui.close();
                        }
                    });
                    theme::icon_in(ui, Icon::User, 16.0, p.ok);
                } else {
                    if ui
                        .button("Sign in")
                        .on_hover_text("Sign in with your PJDesk account for this network and its servers.")
                        .clicked()
                    {
                        self.show_net_sign_in = true;
                        self.net_temporary = true;
                        self.net_enroll_only = false;
                        self.status.clear();
                    }
                    theme::icon_in(ui, Icon::User, 16.0, p.muted);
                }
            });
        });
    }

    pub(super) fn body(&mut self, ui: &mut egui::Ui) {
        if self.cfg.effective_server().trim().is_empty() || self.show_net_sign_in {
            self.sign_in_panel(ui);
            return;
        }
        let width = ui.available_width();
        // Narrow windows stack the connect form above the list. The list pins
        // Connect and Menu to its own right edge at every width.
        let split = desk_split(width);
        if split.stacked {
            ScrollArea::vertical().id_salt("stacked").auto_shrink([false, false]).show(ui, |ui| {
                self.support_card(ui);
                ui.add_space(12.0);
                self.remote(ui);
                ui.add_space(12.0);
                self.rail(ui);
                ui.add_space(12.0);
                self.computer_list(ui);
            });
            return;
        }
        // Place the two columns in the space the window actually has. A horizontal
        // layout would let the left column's minimum width push Connect and Menu
        // past the right edge.
        let rect = ui.available_rect_before_wrap();
        let gap = 12.0;
        let left_w = split.left.min((rect.width() - gap - 280.0).max(0.0));
        let right_x = rect.left() + left_w + gap;
        let left_rect = egui::Rect::from_min_max(rect.left_top(), egui::pos2(right_x - gap, rect.bottom()));
        let right_rect = egui::Rect::from_min_max(egui::pos2(right_x.min(rect.right()), rect.top()), rect.right_bottom());
        let mut left_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("connect-col-wrap")
                .max_rect(left_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        left_ui.set_clip_rect(left_rect);
        ScrollArea::vertical().id_salt("connect-col").auto_shrink([false, false]).show(&mut left_ui, |ui| {
            ui.set_max_width(left_rect.width());
            self.support_card(ui);
            ui.add_space(12.0);
            self.remote(ui);
            ui.add_space(12.0);
            self.rail(ui);
        });
        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("saved-col-wrap")
                .max_rect(right_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        right_ui.set_clip_rect(right_rect);
        right_ui.set_max_width(right_rect.width());
        self.computer_list(&mut right_ui);
        ui.advance_cursor_after_rect(rect);
    }

    fn support_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let (code, password, id, _) = self.support_view();
        let online = pjdesk_proto::is_valid_id(&id) && !self.cfg.effective_server().trim().is_empty();
        theme::card(ui, |ui| {
            widgets::heading(ui, "LET SOMEONE CONNECT");
            ui.add_space(4.0);
            if !online {
                ui.label(widgets::muted(
                    ui,
                    "This computer is not on a network yet, so a support code cannot be used.",
                ));
            } else {
            ui.label(widgets::muted(
                ui,
                "Give them this code and the temporary password. They connect without an Allow prompt. This is not the permanent password. This computer still shows who connected. A code lasts about 10 minutes if it is not used, and a new one is made when the session ends.",
            ));
            ui.add_space(8.0);
            match code {
                Some(code) => {
                    ui.label(RichText::new(crate::network::group_digits(&code)).size(28.0).strong().monospace().color(p.text));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let shown = match &password {
                            Some(pw) if self.show_support_password => pw.clone(),
                            Some(pw) => "•".repeat(pw.chars().count().max(6)),
                            None => "—".into(),
                        };
                        ui.label(RichText::new(shown).size(18.0).monospace().color(p.text));
                        if password.is_some() {
                            let eye = if self.show_support_password { Icon::EyeOff } else { Icon::Eye };
                            let tip = if self.show_support_password { "Hide password" } else { "Show password" };
                            if widgets::icon_button(ui, eye, tip).clicked() {
                                self.show_support_password = !self.show_support_password;
                            }
                        }
                        if widgets::icon_button(ui, Icon::Refresh, "New code and password").clicked() {
                            self.refresh_support_pair();
                        }
                    });
                    ui.label(RichText::new("Temporary password").color(p.muted).size(12.5));
                }
                None => {
                    widgets::spinner(ui, p.muted);
                    ui.label(RichText::new("Preparing a code").color(p.muted));
                }
            }
            }
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

    fn sign_in_panel(&mut self, ui: &mut egui::Ui) {
        let p = theme::paint(ui);
        let mut go = false;
        let mut enroll_only = false;
        let mut cancel = false;
        ui.vertical_centered(|ui| {
            ui.set_max_width(440.0);
            ui.add_space(24.0);
            ui.label(RichText::new("Sign in to your network").size(26.0).strong().color(p.text));
            ui.add_space(4.0);
            let intro = match self.net_mode {
                2 if self.net_temporary => {
                    "Paste a network key. This computer is added to your account. This sign-in ends when you close PJDesk."
                }
                2 => "Paste a network key from My network or the PJDesk Server window. This computer enrolls and stays signed in after a restart.",
                1 => "Enter an 8-digit code from someone on this network. It works once, lasts one hour, and you do not need a PJDesk account. An older enrollment code still works in this box.",
                _ => "Your PJDesk account sets the servers for this network. It does not connect you to a computer, and it is not that computer's password.",
            };
            ui.label(widgets::muted(ui, intro));
            ui.add_space(16.0);
        });
        ui.vertical_centered(|ui| {
            ui.set_max_width(440.0);
            theme::card(ui, |ui| {
                if self.net_mode == 2 {
                    widgets::field_label(ui, "Network key");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.net_blob)
                            .hint_text("pjdesk-net1:…")
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    let embedded = pjdesk_proto::netkey::is_embedded(&self.net_blob);
                    if !embedded {
                        ui.add_space(8.0);
                        widgets::field_label(ui, "Passphrase");
                        widgets::password_field(ui, &mut self.net_key_pass, "Passphrase", &mut self.net_key_show);
                        ui.label(widgets::muted(
                            ui,
                            "Only if the key was created with a passphrase. Leave this empty for a self-contained key.",
                        ));
                    }
                    if embedded {
                        ui.add_space(6.0);
                        ui.colored_label(p.warn, "Anyone who has this network key can join.");
                    }
                    if self.net_key_account || self.net_error.contains("specific account") {
                        self.net_key_account = true;
                        ui.add_space(8.0);
                        widgets::field_label(ui, "Account email");
                        widgets::account_email_field(ui, &mut self.net_email);
                        ui.add_space(8.0);
                        widgets::field_label(ui, "Account password");
                        widgets::account_password_field(ui, &mut self.net_pass, &mut self.net_show);
                    }
                } else if self.net_mode == 0 {
                    widgets::field_label(ui, "Email");
                    widgets::account_email_field(ui, &mut self.net_email);
                    ui.add_space(8.0);
                    widgets::field_label(ui, "PJDesk account password");
                    widgets::account_password_field(ui, &mut self.net_pass, &mut self.net_show);
                } else {
                    widgets::field_label(ui, "8-digit code");
                    widgets::text_field(ui, &mut self.net_code, "1234 5678", true);
                    ui.label(widgets::muted(
                        ui,
                        "From Invite on a signed-in PJDesk. One use, one hour. An older enrollment code from pjdesk.com still works here.",
                    ));
                }
                ui.add_space(8.0);
                widgets::checkbox(
                    ui,
                    &mut self.net_temporary,
                    "Sign me out when I'm done (use on someone else's computer)",
                );
                ui.label(widgets::muted(
                    ui,
                    "This computer is added to your account either way. When this is checked, the sign-in ends when you close PJDesk and is not kept on this computer.",
                ));
                ui.add_space(8.0);
                if !self.net_error.is_empty() {
                    ui.colored_label(p.warn, &self.net_error);
                    ui.add_space(6.0);
                }
                if self.net_offer
                    && !self.net_busy
                    && ui.button(format!("Use this computer's current server ({}) for my network", self.cfg.effective_server())).clicked()
                {
                    self.claim_current_server();
                }
                let label = if self.net_busy {
                    "Signing in…"
                } else if self.net_mode == 2 {
                    "Use network key"
                } else if self.net_mode == 1 {
                    "Join"
                } else {
                    "Sign in"
                };
                if widgets::accent_button(ui, label, egui::vec2(ui.available_width(), 40.0)) && !self.net_busy {
                    go = true;
                }
                if self.net_mode == 0 && !self.net_busy {
                    ui.add_space(4.0);
                    let link = ui.add(
                        egui::Button::new(
                            RichText::new("Just add this computer to my account").underline().color(p.accent),
                        )
                        .frame(false),
                    );
                    if link.clicked() {
                        enroll_only = true;
                    }
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if self.net_mode != 0 && ui.button("Use email and password").clicked() && !self.net_busy {
                        self.net_mode = 0;
                        self.net_error.clear();
                        self.net_offer = false;
                        self.net_login.clear();
                    }
                    if self.net_mode != 1 && ui.button("Join with an 8-digit code").clicked() && !self.net_busy {
                        self.net_mode = 1;
                        self.net_error.clear();
                        self.net_offer = false;
                        self.net_login.clear();
                    }
                    if self.net_mode != 2 && ui.button("Use a network key").clicked() && !self.net_busy {
                        self.net_mode = 2;
                        self.net_error.clear();
                        self.net_offer = false;
                        self.net_login.clear();
                    }
                    if self.show_net_sign_in && !self.cfg.effective_server().is_empty() && ui.button("Cancel").clicked()
                    {
                        cancel = true;
                    }
                });
            });
        });
        if cancel {
            self.show_net_sign_in = false;
        }
        if enroll_only {
            self.net_enroll_only = true;
            self.start_network_sign_in();
        } else if go {
            self.net_enroll_only = false;
            self.start_network_sign_in();
        }
    }

    pub(super) fn import_prompt(&mut self, ctx: &egui::Context) {
        if self.import_blob.is_none() {
            return;
        }
        let blob = self.import_blob.clone().unwrap_or_default();
        let needs = pjdesk_proto::netkey::needs_passphrase(&blob);
        let embedded = pjdesk_proto::netkey::is_embedded(&blob);
        let confirm = self.import_confirm;
        let hint = self.import_hint.clone();
        let mut go = false;
        let mut cancel = false;
        let mut open = true;
        egui::Window::new("Import server config")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                if confirm {
                    ui.label(RichText::new("Import this server config?").strong());
                    ui.add_space(4.0);
                }
                if !hint.is_empty() {
                    ui.label(widgets::muted(ui, &hint));
                }
                if embedded {
                    ui.add_space(6.0);
                    ui.colored_label(theme::paint(ui).warn, "Anyone who has this network key can join.");
                }
                if needs {
                    ui.add_space(8.0);
                    widgets::field_label(ui, "Passphrase");
                    widgets::password_field(ui, &mut self.import_pass, "Passphrase", &mut self.import_show);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Import server config").clicked() {
                        go = true;
                    }
                });
            });
        if cancel || !open {
            self.dismiss_import();
        } else if go {
            if needs && self.import_pass.trim().is_empty() {
                self.import_hint = "Enter the passphrase.".into();
                return;
            }
            let pass = self.import_pass.clone();
            self.begin_import(blob, pass);
        }
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
        let reach = if self.engine.service_running {
            self.svc_status.reach.clone()
        } else {
            self.engine.shared.reach()
        };
        let server = if reach.is_empty() { server } else { format!("{server} · {reach}") };
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
                    let width = (ui.available_width() - menu_w - 8.0).max(120.0);
                    let before = self.connect_id.clone();
                    let mut output = egui::TextEdit::singleline(&mut self.connect_id)
                        .hint_text("123 456 789")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(width)
                        .margin(egui::Margin::symmetric(12, 10))
                        .show(ui);
                    if output.response.changed() {
                        let after_caret = output
                            .state
                            .cursor
                            .char_range()
                            .map(|range| usize::from(range.primary.index))
                            .unwrap_or_else(|| self.connect_id.chars().count());
                        let (formatted, caret) = crate::idfmt::reformat_id_change(&before, &self.connect_id, after_caret, 9);
                        self.connect_id = formatted;
                        let cursor = egui::text::CCursor::new(caret);
                        output.state.cursor.set_char_range(Some(egui::text::CCursorRange::one(cursor)));
                        output.state.store(ui.ctx(), output.response.id);
                    }
                    let resp = output.response;
                    let recent = self.recent_ids();
                    ui.menu_button("Recent", |ui| {
                        if recent.is_empty() {
                            ui.label(widgets::muted(ui, "No recent computers"));
                        }
                        for id in recent {
                            let label = self.row_title(&id);
                            if ui.button(label).clicked() {
                                self.connect_id = crate::idfmt::format_typed_id(&id);
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Recent computers");
                    resp
                })
                .inner;

            ui.add_space(8.0);
            ui.label(widgets::muted(
                ui,
                "Enter the ID and click Connect. A saved password is used on its own. Otherwise PJDesk asks after that computer answers.",
            ));
            ui.add_space(8.0);
            let enter = id_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let connect = widgets::accent_button(ui, "Connect", Vec2::new(ui.available_width(), 40.0));
            if connect || enter {
                let id = self.connect_id.clone();
                self.connect_to(&id, None, false);
            }
            if ui.button("Request view access").clicked() {
                let id = self.connect_id.clone();
                self.request_view(&id);
            }
            ui.label(widgets::muted(
                ui,
                "Request view access asks that phone to allow the screen. It uses your pjdesk.com account when you are signed in.",
            ));

            ui.add_space(12.0);
            widgets::field_label(ui, "Or use their support code");
            widgets::text_field(ui, &mut self.support_code_in, "1234 5678", true);
            ui.add_space(6.0);
            widgets::password_field(ui, &mut self.support_pass_in, "Temporary password", &mut self.support_pass_show);
            if !self.support_error.is_empty() {
                ui.colored_label(p.warn, &self.support_error);
            }
            let support_label = if self.support_busy { "Connecting…" } else { "Connect with code" };
            if widgets::accent_button(ui, support_label, Vec2::new(ui.available_width(), 36.0)) && !self.support_busy {
                self.start_support_connect();
            }
            ui.label(widgets::muted(
                ui,
                "From Let someone connect on their computer. The code and the temporary password are both required.",
            ));

            ui.add_space(12.0);
            widgets::field_label(ui, "Or connect directly");
            widgets::text_field(ui, &mut self.direct_addr, "192.168.1.20", false);
            ui.horizontal(|ui| {
                if ui.button("Connect to address").clicked() {
                    let addr = self.direct_addr.clone();
                    self.connect_direct(&addr);
                }
                if ui.button("Find on this network").clicked() {
                    self.find_on_lan();
                }
            });
        });
    }

    pub(super) fn invite_window(&mut self, ctx: &egui::Context) {
        if !self.show_invite {
            return;
        }
        let mut open = true;
        let mut create = false;
        let mut revoke = false;
        let code = self.join_code.clone();
        let expires = self.join_expires;
        let busy = self.join_busy;
        let error = self.join_error.clone();
        let has_id = !self.join_id.is_empty();
        egui::Window::new("Invite someone")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                let p = theme::paint(ui);
                if !code.is_empty() {
                    ui.label(RichText::new(crate::network::group_digits(&code)).size(32.0).strong().monospace().color(p.text));
                    ui.add_space(6.0);
                    ui.label(format!(
                        "Expires in {}. One use. They do not need a PJDesk account. They choose Join with an 8-digit code.",
                        crate::network::remaining_label(expires, super::unix_now())
                    ));
                } else if has_id {
                    ui.label(format!(
                        "A code is already active and expires in {}. The digits were shown once. Revoke it, or create a new one (that replaces it).",
                        crate::network::remaining_label(expires, super::unix_now())
                    ));
                } else {
                    ui.label("Create an 8-digit code. It works once, lasts one hour, and the other person does not need a PJDesk account.");
                }
                if !error.is_empty() {
                    ui.add_space(6.0);
                    ui.colored_label(p.warn, &error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let create_label = if busy { "Working…" } else if code.is_empty() && !has_id { "Create code" } else { "Create a new code" };
                    if ui.add_enabled(!busy, egui::Button::new(create_label)).clicked() {
                        create = true;
                    }
                    if has_id && ui.add_enabled(!busy, egui::Button::new("Revoke")).clicked() {
                        revoke = true;
                    }
                });
            });
        if !open {
            self.show_invite = false;
        }
        if create {
            self.start_invite(super::InviteKind::Create);
        }
        if revoke {
            self.start_invite(super::InviteKind::Revoke);
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
        egui::Window::new("Older local account")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .constrain(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(widgets::muted(
                    ui,
                    "An older account stored on the server. It is not your PJDesk account, and it is not the password for a computer.",
                ));
                ui.add_space(8.0);
                if self.cfg.server.is_none() {
                    ui.colored_label(p.warn, "Set your server in Settings first.");
                }
                ui.label("Username");
                ui.text_edit_singleline(&mut self.login_user);
                ui.add_space(8.0);
                ui.label("Local account password");
                let pw = widgets::password_field(ui, &mut self.login_pass, "Local account password", &mut self.show_login_pass);
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

    /// Nickname, then the name this computer chose, then its hostname.
    pub(super) fn row_title(&self, id: &str) -> String {
        let hostname = self.cfg.settings.peers.get(id).map(|n| n.hostname.trim().to_string()).unwrap_or_default();
        let display = self.cfg.settings.peers.get(id).map(|n| n.display_name.trim().to_string()).unwrap_or_default();
        let alias = self.cfg.address_book.iter().find(|e| e.id == id).map(|e| e.alias.clone()).unwrap_or_default();
        let nick = crate::config::nickname_text(&alias, id, &hostname);
        if !nick.is_empty() {
            return nick;
        }
        if !display.is_empty() {
            return display;
        }
        if !hostname.is_empty() {
            return hostname;
        }
        format_id(id)
    }
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
