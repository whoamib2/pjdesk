//! System-tray icon for the PJDesk Server window.
//!
//! Windows-only (the management window is Windows-only). The service runs in
//! session 0 and cannot show an icon; this is the per-user window. Tray events
//! wake the UI with `request_repaint`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user asked for via the tray.
pub enum Action {
    Show,
    CheckUpdates,
    Promote,
    Demote,
    SyncNow,
    Quit,
}

static MENU_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ICON_CLICKED: AtomicBool = AtomicBool::new(false);

pub struct Tray {
    icon: TrayIcon,
    show_id: String,
    updates_id: String,
    promote_id: String,
    demote_id: String,
    sync_id: String,
    quit_id: String,
}

impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let icon = Icon::from_rgba(include_bytes!("../assets/icon_server_32.rgba").to_vec(), 32, 32).ok()?;

        let menu = Menu::new();
        let show = MenuItem::new("Open PJDesk Server", true, None);
        let updates = MenuItem::new("Check for updates now", true, None);
        let promote = MenuItem::new("Make this the primary", true, None);
        let demote = MenuItem::new("Step down to standby", true, None);
        let sync = MenuItem::new("Cloud sync now", true, None);
        let quit = MenuItem::new("Exit tray", true, None);
        menu.append(&show).ok()?;
        menu.append(&updates).ok()?;
        menu.append(&promote).ok()?;
        menu.append(&demote).ok()?;
        menu.append(&sync).ok()?;
        menu.append(&quit).ok()?;
        let show_id = show.id().0.clone();
        let updates_id = updates.id().0.clone();
        let promote_id = promote.id().0.clone();
        let demote_id = demote.id().0.clone();
        let sync_id = sync.id().0.clone();
        let quit_id = quit.id().0.clone();

        let c1 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            MENU_IDS.lock().unwrap().push(e.id.0.clone());
            c1.request_repaint();
        }));
        let c2 = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if matches!(e, TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }) {
                ICON_CLICKED.store(true, Ordering::SeqCst);
                c2.request_repaint();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_tooltip("PJDesk Server")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .ok()?;
        Some(Self { icon, show_id, updates_id, promote_id, demote_id, sync_id, quit_id })
    }

    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }

    pub fn poll(&self) -> Vec<Action> {
        let mut out = Vec::new();
        if ICON_CLICKED.swap(false, Ordering::SeqCst) {
            out.push(Action::Show);
        }
        for id in MENU_IDS.lock().unwrap().drain(..) {
            if id == self.show_id {
                out.push(Action::Show);
            } else if id == self.updates_id {
                out.push(Action::CheckUpdates);
            } else if id == self.promote_id {
                out.push(Action::Promote);
            } else if id == self.demote_id {
                out.push(Action::Demote);
            } else if id == self.sync_id {
                out.push(Action::SyncNow);
            } else if id == self.quit_id {
                out.push(Action::Quit);
            }
        }
        out
    }
}
