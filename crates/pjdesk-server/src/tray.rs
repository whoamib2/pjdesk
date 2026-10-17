//! System-tray icon for the PJDesk Server window (minimize-to-tray).
//!
//! Windows-only (the management window is Windows-only). Tray events wake the
//! UI with `request_repaint`, so Show/Quit work even while the window is hidden.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user asked for via the tray.
pub enum Action {
    Show,
    Quit,
}

static MENU_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ICON_CLICKED: AtomicBool = AtomicBool::new(false);

pub struct Tray {
    _icon: TrayIcon,
    show_id: String,
    quit_id: String,
}

impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let icon = Icon::from_rgba(include_bytes!("../assets/icon_server_32.rgba").to_vec(), 32, 32).ok()?;

        let menu = Menu::new();
        let show = MenuItem::new("Show PJDesk Server", true, None);
        let quit = MenuItem::new("Quit PJDesk Server", true, None);
        menu.append(&show).ok()?;
        menu.append(&quit).ok()?;
        let show_id = show.id().0.clone();
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
        Some(Self { _icon: icon, show_id, quit_id })
    }

    pub fn poll(&self) -> Vec<Action> {
        let mut out = Vec::new();
        if ICON_CLICKED.swap(false, Ordering::SeqCst) {
            out.push(Action::Show);
        }
        for id in MENU_IDS.lock().unwrap().drain(..) {
            if id == self.show_id {
                out.push(Action::Show);
            } else if id == self.quit_id {
                out.push(Action::Quit);
            }
        }
        out
    }
}
