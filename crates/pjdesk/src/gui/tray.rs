//! System-tray icon with a Show/Quit menu, integrated with the eframe loop.
//!
//! Tray events arrive on background callbacks; each one wakes the UI with
//! `request_repaint` so the app processes them even while hidden in the tray.

/// What the user asked for via the tray.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Action {
    Show,
    Update,
    Revoke,
    Quit,
}

#[cfg(windows)]
pub use imp::Tray;
#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::Action;

    static MENU_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static ICON_CLICKED: AtomicBool = AtomicBool::new(false);

    /// Keeps the tray icon alive and remembers its menu item ids.
    pub struct Tray {
        _icon: TrayIcon,
        show_id: String,
        update_id: String,
        revoke_id: String,
        quit_id: String,
    }

    impl Tray {
        /// Builds the tray icon, or returns `None` if the system has no tray.
        pub fn new(ctx: &egui::Context) -> Option<Self> {
            let icon = Icon::from_rgba(include_bytes!("../../assets/icon_32.rgba").to_vec(), 32, 32).ok()?;

            let menu = Menu::new();
            let show = MenuItem::new("Show PJDesk", true, None);
            let update = MenuItem::new("Check for updates now", true, None);
            let revoke = MenuItem::new("Revoke access", true, None);
            let quit = MenuItem::new("Quit PJDesk", true, None);
            menu.append(&show).ok()?;
            menu.append(&update).ok()?;
            menu.append(&revoke).ok()?;
            menu.append(&quit).ok()?;
            let show_id = show.id().0.clone();
            let update_id = update.id().0.clone();
            let revoke_id = revoke.id().0.clone();
            let quit_id = quit.id().0.clone();

            // Wake the UI whenever a tray event arrives, so Show/Quit work even
            // while the window is hidden.
            let show_now = show_id.clone();
            let c1 = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if e.id.0 == show_now {
                    crate::winshow::activate("Open", "PJDesk", super::super::instance::holds_window());
                }
                MENU_IDS.lock().unwrap().push(e.id.0.clone());
                c1.request_repaint();
            }));
            let c2 = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                let click = match e {
                    TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                        Some(pjdesk_proto::trayshow::Click::Left)
                    }
                    TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => Some(pjdesk_proto::trayshow::Click::LeftDouble),
                    _ => None,
                };
                let Some(click) = click else { return };
                if !pjdesk_proto::trayshow::opens_window(click) {
                    return;
                }
                let reason = if click == pjdesk_proto::trayshow::Click::LeftDouble { "double-click" } else { "left click" };
                crate::winshow::activate(reason, "PJDesk", super::super::instance::holds_window());
                ICON_CLICKED.store(true, Ordering::SeqCst);
                c2.request_repaint();
            }));

            let icon = TrayIconBuilder::new()
                .with_tooltip("PJDesk")
                .with_icon(icon)
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;
            Some(Self { _icon: icon, show_id, update_id, revoke_id, quit_id })
        }

        pub fn set_tooltip(&self, text: &str) {
            let _ = self._icon.set_tooltip(Some(text));
        }

        /// Drains any pending tray actions.
        pub fn poll(&self) -> Vec<Action> {
            let mut out = Vec::new();
            let clicked = ICON_CLICKED.swap(false, Ordering::SeqCst);
            let second = super::super::instance::show_requested();
            if second {
                crate::winshow::activate("second launch", "PJDesk", super::super::instance::holds_window());
            }
            if clicked || second {
                out.push(Action::Show);
            }
            for id in MENU_IDS.lock().unwrap().drain(..) {
                if id == self.show_id {
                    out.push(Action::Show);
                } else if id == self.update_id {
                    out.push(Action::Update);
                } else if id == self.revoke_id {
                    out.push(Action::Revoke);
                } else if id == self.quit_id {
                    out.push(Action::Quit);
                }
            }
            out
        }
    }
}

#[cfg(not(windows))]
pub use stub::Tray;
#[cfg(not(windows))]
mod stub {
    use eframe::egui;

    use super::Action;

    pub struct Tray;
    impl Tray {
        pub fn new(_ctx: &egui::Context) -> Option<Self> {
            None
        }
        pub fn poll(&self) -> Vec<Action> {
            Vec::new()
        }
        pub fn set_tooltip(&self, _text: &str) {}
    }
}
